import app as service


class FakeResponse:
    def __init__(self, status_code=200, payload=None):
        self.status_code = status_code
        self._payload = payload or {}

    def get_json(self):
        return self._payload


def test_health_is_process_only():
    response = service.app.test_client().get('/health')
    assert response.status_code == 200
    assert response.get_json() == {'status': 'ok'}


def test_readiness_fails_closed_when_vault_is_unavailable(monkeypatch):
    def unavailable(*args, **kwargs):
        raise service.requests.RequestException('offline')

    monkeypatch.setattr(service.requests, 'get', unavailable)
    response = service.app.test_client().get('/ready')
    assert response.status_code == 503
    assert response.get_json()['status'] == 'not_ready'


def test_prometheus_metrics_do_not_include_secret_material(monkeypatch):
    monkeypatch.setattr(
        service,
        'metrics',
        lambda: FakeResponse(
            payload={
                'status': 'ok',
                'accessor_count': 2,
                'issuance_event_count': 3,
                'secret_id': 'must-not-appear',
            }
        ),
    )
    response = service.app.test_client().get('/metrics/prometheus')
    body = response.get_data(as_text=True)
    assert response.status_code == 200
    assert 'un1c0_admin_status 1' in body
    assert 'un1c0_admin_accessor_count 2' in body
    assert 'must-not-appear' not in body
