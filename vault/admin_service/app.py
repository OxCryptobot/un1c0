import os
import json
from flask import Flask, Response, request, jsonify
import requests
import logging
import datetime

app = Flask(__name__)

VAULT_ADDR = os.environ.get('VAULT_ADDR', 'http://vault:8200')
VAULT_TOKEN = os.environ.get('VAULT_TOKEN', 'root-token')
ADMIN_API_KEY = os.environ.get('ADMIN_API_KEY', 'changeme')
APPROLE_NAME = os.environ.get('APPROLE_NAME', 'master-key-approle')
HTTP_TIMEOUT_SECONDS = float(os.environ.get('HTTP_TIMEOUT_SECONDS', '3'))

KV_ACCESSORS_PATH = 'kv/data/approle_accessors'

headers = {
    'X-Vault-Token': VAULT_TOKEN
}

# Logging setup: write to container-mounted `/logs` so the host/CI can collect logs.
LOG_DIR = os.environ.get('ADMIN_LOG_DIR', '/logs')
try:
    os.makedirs(LOG_DIR, exist_ok=True)
except PermissionError:
    LOG_DIR = os.path.join(os.environ.get('TMPDIR', '/tmp'), 'un1c0-admin-logs')
    os.makedirs(LOG_DIR, exist_ok=True)
LOG_FILE = os.path.join(LOG_DIR, 'admin_service.log')
logging.basicConfig(
    level=logging.INFO,
    handlers=[logging.FileHandler(LOG_FILE), logging.StreamHandler()],
    format='%(asctime)s %(levelname)s %(message)s',
)
logger = logging.getLogger('admin_service')


def vault_post(path, data=None):
    url = VAULT_ADDR.rstrip('/') + '/v1/' + path.lstrip('/')
    r = requests.post(url, headers=headers, json=data, timeout=HTTP_TIMEOUT_SECONDS)
    r.raise_for_status()
    return r.json()


def vault_get(path):
    url = VAULT_ADDR.rstrip('/') + '/v1/' + path.lstrip('/')
    r = requests.get(url, headers=headers, timeout=HTTP_TIMEOUT_SECONDS)
    r.raise_for_status()
    return r.json()


@app.route('/health', methods=['GET'])
def health():
    return jsonify({'status': 'ok'})


@app.route('/ready', methods=['GET'])
def ready():
    try:
        response = requests.get(
            VAULT_ADDR.rstrip('/') + '/v1/sys/health',
            headers=headers,
            timeout=HTTP_TIMEOUT_SECONDS,
        )
        if response.status_code != 200:
            return jsonify({'status': 'not_ready', 'vault_status': response.status_code}), 503
    except requests.RequestException:
        return jsonify({'status': 'not_ready'}), 503
    return jsonify({'status': 'ready'})


@app.route('/issue-secret-id', methods=['POST'])
def issue_secret_id():
    # Enforce mTLS: nginx should forward the client DN in `X-SSL-CLIENT-S-DN` header.
    # Reject requests that do not present a client certificate to the TLS terminator.
    client_dn = request.headers.get('X-SSL-Client-S-Dn') or request.environ.get('HTTP_X_SSL_CLIENT_S_DN')
    if not client_dn:
        logger.warning('Rejected request: mTLS client cert required')
        return jsonify({'error': 'unauthorized'}), 401

    payload = request.json or {}
    role = payload.get('role', APPROLE_NAME)

    # Create secret-id
    # Create secret-id (plain response)
    resp = vault_post(f'auth/approle/role/{role}/secret-id')
    # resp contains secret_id and secret_id_accessor
    secret_id = resp.get('data', {}).get('secret_id')
    accessor = resp.get('data', {}).get('secret_id_accessor')

    # Store accessor in KV for tracking (append to list)
    try:
        existing = vault_get(KV_ACCESSORS_PATH)
        data = existing.get('data', {}).get('data', {})
        arr = data.get('accessors', [])
    except requests.exceptions.HTTPError:
        arr = []

    arr.append({'accessor': accessor})
    vault_post('kv/data/approle_accessors', data={'data': {'accessors': arr}})

    # Audit: record issuance event
    event = {
        'ts': datetime.datetime.utcnow().isoformat() + 'Z',
        'action': 'issue-secret-id',
        'role': role,
        'accessor': accessor
    }
    try:
        # append to issuance_events KV
        try:
            existing = vault_get('kv/data/issuance_events')
            events = existing.get('data', {}).get('data', {}).get('events', [])
        except requests.exceptions.HTTPError:
            events = []
        events.append(event)
        vault_post('kv/data/issuance_events', data={'data': {'events': events}})
    except Exception as e:
        logger.exception('Failed to record issuance event: %s', e)

    logger.info('Issued secret_id accessor=%s for role=%s', accessor, role)

    return jsonify({'secret_id': secret_id, 'accessor': accessor})


@app.route('/issue-wrapped-secret-id', methods=['POST'])
def issue_wrapped_secret_id():
    # Require mTLS: nginx should forward the client DN in `X-SSL-CLIENT-S-DN` header.
    # Reject requests that do not present a client certificate to the TLS terminator.
    client_dn = request.headers.get('X-SSL-Client-S-Dn') or request.environ.get('HTTP_X_SSL_CLIENT_S_DN')
    if not client_dn:
        logger.warning('Rejected request: mTLS client cert required')
        return jsonify({'error': 'unauthorized'}), 401

    payload = request.json or {}
    role = payload.get('role', APPROLE_NAME)
    wrap_ttl = payload.get('wrap_ttl', '60s')

    # Request a wrapped secret-id from Vault (response wrapping)
    url = VAULT_ADDR.rstrip('/') + f'/v1/auth/approle/role/{role}/secret-id'
    # Use the X-Vault-Wrap-Ttl header to request response wrapping
    headers_wrap = headers.copy()
    headers_wrap['X-Vault-Wrap-Ttl'] = wrap_ttl
    r = requests.post(url, headers=headers_wrap)
    # The wrapped response does not include the inner secret in body; instead Vault returns a wrapping token
    r.raise_for_status()
    wrapped = r.json() or {}
    # wrapped may include wrap_info (when using response wrapping) or data (when not wrapped)
    wrap_info = wrapped.get('wrap_info') or {}
    wrap_token = wrap_info.get('token')
    accessor = (wrapped.get('data') or {}).get('secret_id_accessor') or wrap_info.get('accessor')

    if not wrap_token:
        logger.error('Vault did not return a wrap_token for wrapped issuance: %s', wrapped)
        return jsonify({'error': 'failed to obtain wrapped token from Vault'}), 500

    # Store accessor for tracking (we store accessor but do not store the secret_id)
    try:
        existing = vault_get(KV_ACCESSORS_PATH)
        data = existing.get('data', {}).get('data', {})
        arr = data.get('accessors', [])
    except requests.exceptions.HTTPError:
        arr = []

    arr.append({'accessor': accessor})
    vault_post('kv/data/approle_accessors', data={'data': {'accessors': arr}})

    # Audit event for wrapped issuance
    event = {
        'ts': datetime.datetime.utcnow().isoformat() + 'Z',
        'action': 'issue-wrapped-secret-id',
        'role': role,
        'accessor': accessor,
        'wrap_ttl': wrap_ttl
    }
    try:
        try:
            existing = vault_get('kv/data/issuance_events')
            events = existing.get('data', {}).get('data', {}).get('events', [])
        except requests.exceptions.HTTPError:
            events = []
        events.append(event)
        vault_post('kv/data/issuance_events', data={'data': {'events': events}})
    except Exception:
        logger.exception('Failed to record wrapped issuance event')

    logger.info('Issued wrapped secret accessor=%s role=%s wrap_ttl=%s', accessor, role, wrap_ttl)

    return jsonify({'wrap_token': wrap_token, 'accessor': accessor})


@app.route('/revoke-accessors', methods=['POST'])
def revoke_accessors():
    # Require mTLS client cert header
    client_dn = request.headers.get('X-SSL-CLIENT-S-DN') or request.environ.get('HTTP_X_SSL_CLIENT_S_DN')
    if not client_dn:
        return jsonify({'error': 'mTLS client cert required'}), 401

    # Require break-glass token to be configured on the admin service host and valid
    BREAK_GLASS_TOKEN = os.environ.get('BREAK_GLASS_TOKEN')
    if not BREAK_GLASS_TOKEN:
        return jsonify({'error': 'break-glass token not configured on server; cannot revoke accessors'}), 403
    # Validate break-glass token by attempting to read master_key
    try:
        r = requests.get(VAULT_ADDR.rstrip('/') + '/v1/kv/data/master_key', headers={'X-Vault-Token': BREAK_GLASS_TOKEN})
        if r.status_code != 200:
            return jsonify({'error': 'break-glass token invalid or expired; cannot revoke accessors'}), 403
    except Exception:
        return jsonify({'error': 'failed to validate break-glass token'}), 500

    payload = request.json or {}
    role = payload.get('role', APPROLE_NAME)
    keep_latest = payload.get('keep_latest', True)

    # Read stored accessors
    try:
        existing = vault_get(KV_ACCESSORS_PATH)
        data = existing.get('data', {}).get('data', {})
        arr = data.get('accessors', [])
    except requests.exceptions.HTTPError:
        arr = []

    if not arr:
        return jsonify({'revoked': 0})

    revoked_count = 0
    # If keep_latest, skip last element
    to_revoke = arr[:-1] if keep_latest and len(arr) > 1 else arr
    for item in to_revoke:
        accessor = item.get('accessor')
        if not accessor:
            continue
        # Revoke by accessor
        try:
            vault_post(f'auth/approle/role/{role}/secret-id-accessor/destroy', data={'secret_id_accessor': accessor})
            revoked_count += 1
        except requests.exceptions.HTTPError:
            # continue on errors
            continue

    # Update stored accessors to keep only the last one (if requested)
    remaining = arr[-1:] if keep_latest and len(arr) > 0 else []
    vault_post('kv/data/approle_accessors', data={'data': {'accessors': remaining}})

    return jsonify({'revoked': revoked_count, 'remaining': remaining})


@app.route('/rotate-master-key', methods=['POST'])
def rotate_master_key():
    # Require mTLS authentication
    client_dn = request.headers.get('X-SSL-Client-S-Dn') or request.environ.get('HTTP_X_SSL_CLIENT_S_DN')
    if not client_dn:
        logger.warning('Rejected rotate-master-key: mTLS client cert required')
        return jsonify({'error': 'unauthorized'}), 401

    new_key = os.popen('openssl rand -hex 32').read().strip()
    expiry = request.json.get('expiry') if request.json else None
    if not expiry:
        import datetime
        expiry = (datetime.datetime.utcnow() + datetime.timedelta(days=1)).isoformat() + 'Z'

    # Write new key to Vault KV v2
    vault_post('kv/data/master_key', data={'data': {'key': new_key, 'expiry': expiry}})

    # Revoke old AppRole secret-ids except the latest
    try:
        revoke_resp = revoke_all_except_latest_internal()
    except Exception:
        revoke_resp = {'revoked': 0}

    return jsonify({'new_key_set': True, 'revoked': revoke_resp.get('revoked', 0)})


def revoke_all_except_latest_internal():
    # Read stored accessors
    try:
        existing = vault_get(KV_ACCESSORS_PATH)
        data = existing.get('data', {}).get('data', {})
        arr = data.get('accessors', [])
    except requests.exceptions.HTTPError:
        arr = []

    revoked_count = 0
    if not arr:
        return {'revoked': 0}

    to_revoke = arr[:-1] if len(arr) > 1 else []
    for item in to_revoke:
        accessor = item.get('accessor')
        if not accessor:
            continue
        try:
            vault_post(f'auth/approle/role/{APPROLE_NAME}/secret-id-accessor/destroy', data={'secret_id_accessor': accessor})
            revoked_count += 1
        except Exception:
            continue

    remaining = arr[-1:] if len(arr) > 0 else []
    vault_post('kv/data/approle_accessors', data={'data': {'accessors': remaining}})
    return {'revoked': revoked_count}


@app.route('/metrics', methods=['GET'])
def metrics():
    """Return metrics for monitoring: accessor count, cert expiration, failed auth count."""
    try:
        # Get accessor count
        try:
            existing = vault_get(KV_ACCESSORS_PATH)
            accessor_count = len(existing.get('data', {}).get('data', {}).get('accessors', []))
        except requests.exceptions.HTTPError:
            accessor_count = 0

        # Get issuance event count
        try:
            events_data = vault_get('kv/data/issuance_events')
            issuance_count = len(events_data.get('data', {}).get('data', {}).get('events', []))
        except requests.exceptions.HTTPError:
            issuance_count = 0

        # Check cert expiration (read from environment or mounted cert)
        cert_expiry_days = None
        try:
            import ssl
            import socket
            from datetime import datetime
            # This is a placeholder - in production, check the actual client cert path
            # For now, return a warning if certs are not being monitored
            cert_expiry_days = -1  # Indicates monitoring not implemented
        except Exception:
            cert_expiry_days = None

        return jsonify({
            'accessor_count': accessor_count,
            'issuance_event_count': issuance_count,
            'cert_expiry_days': cert_expiry_days,
            'status': 'ok'
        })
    except Exception as e:
        logger.exception('Failed to generate metrics: %s', e)
        return jsonify({'error': 'failed to generate metrics'}), 500


@app.route('/metrics/prometheus', methods=['GET'])
def metrics_prometheus():
    try:
        payload = metrics().get_json() or {}
        lines = [
            '# HELP un1c0_admin_status Admin service health status.',
            '# TYPE un1c0_admin_status gauge',
            f"un1c0_admin_status {1 if payload.get('status') == 'ok' else 0}",
            '# HELP un1c0_admin_accessor_count Stored AppRole accessor count.',
            '# TYPE un1c0_admin_accessor_count gauge',
            f"un1c0_admin_accessor_count {int(payload.get('accessor_count', 0))}",
            '# HELP un1c0_admin_issuance_event_count Stored issuance event count.',
            '# TYPE un1c0_admin_issuance_event_count gauge',
            f"un1c0_admin_issuance_event_count {int(payload.get('issuance_event_count', 0))}",
        ]
        return Response('\\n'.join(lines) + '\\n', mimetype='text/plain')
    except Exception:
        return Response('un1c0_admin_status 0\\n', status=503, mimetype='text/plain')


@app.route('/issue-job-secret', methods=['POST'])
def issue_job_secret():
    """Issue a job-specific wrapped secret for dynamic per-job authentication."""
    # Require mTLS authentication
    client_dn = request.headers.get('X-SSL-Client-S-Dn') or request.environ.get('HTTP_X_SSL_CLIENT_S_DN')
    if not client_dn:
        logger.warning('Rejected issue-job-secret: mTLS client cert required')
        return jsonify({'error': 'unauthorized'}), 401

    payload = request.json or {}
    job_id = payload.get('job_id', 'unknown')
    wrap_ttl = payload.get('wrap_ttl', '120s')  # Longer TTL for job execution
    role = payload.get('role', APPROLE_NAME)

    # Request wrapped secret-id from Vault
    url = VAULT_ADDR.rstrip('/') + f'/v1/auth/approle/role/{role}/secret-id'
    headers_wrap = headers.copy()
    headers_wrap['X-Vault-Wrap-Ttl'] = wrap_ttl
    r = requests.post(url, headers=headers_wrap)
    r.raise_for_status()
    wrapped = r.json() or {}
    wrap_info = wrapped.get('wrap_info') or {}
    wrap_token = wrap_info.get('token')
    accessor = (wrapped.get('data') or {}).get('secret_id_accessor') or wrap_info.get('accessor')

    if not wrap_token:
        logger.error('Vault did not return wrap_token for job-specific issuance: %s', wrapped)
        return jsonify({'error': 'failed to obtain wrapped token from Vault'}), 500

    # Store accessor with job_id metadata
    try:
        existing = vault_get(KV_ACCESSORS_PATH)
        data = existing.get('data', {}).get('data', {})
        arr = data.get('accessors', [])
    except requests.exceptions.HTTPError:
        arr = []

    arr.append({'accessor': accessor, 'job_id': job_id})
    vault_post('kv/data/approle_accessors', data={'data': {'accessors': arr}})

    # Audit event
    event = {
        'ts': datetime.datetime.utcnow().isoformat() + 'Z',
        'action': 'issue-job-secret',
        'role': role,
        'accessor': accessor,
        'job_id': job_id,
        'wrap_ttl': wrap_ttl
    }
    try:
        try:
            existing = vault_get('kv/data/issuance_events')
            events = existing.get('data', {}).get('data', {}).get('events', [])
        except requests.exceptions.HTTPError:
            events = []
        events.append(event)
        vault_post('kv/data/issuance_events', data={'data': {'events': events}})
    except Exception as e:
        logger.exception('Failed to record job issuance event: %s', e)

    logger.info('Issued job-specific secret accessor=%s job_id=%s role=%s', accessor, job_id, role)
    return jsonify({'wrap_token': wrap_token, 'accessor': accessor, 'job_id': job_id})


if __name__ == '__main__':
    app.run(host='0.0.0.0', port=int(os.environ.get('ADMIN_PORT', '5000')))
