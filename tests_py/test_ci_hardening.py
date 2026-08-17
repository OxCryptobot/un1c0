import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def test_ci_port_allocator_emits_distinct_local_ports_and_namespace():
    env = os.environ | {"GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2"}
    output = subprocess.check_output(
        [str(ROOT / "scripts" / "allocate_ci_ports.sh")],
        cwd=ROOT,
        env=env,
        text=True,
    )
    values = dict(line.split("=", 1) for line in output.strip().splitlines())
    ports = [int(values[name]) for name in ("VAULT_PORT", "ADMIN_PORT", "NGINX_PORT")]
    assert len(set(ports)) == 3
    assert all(1024 < port < 65536 for port in ports)
    assert values["COMPOSE_PROJECT_NAME"] == "un1c0-e2e-123-2"


def test_ci_workflow_uses_isolated_compose_and_unconditional_cleanup():
    workflow = (ROOT / ".github" / "workflows" / "e2e_wrapped_flow.yml").read_text()
    assert "allocate_ci_ports.sh" in workflow
    assert "docker compose -p \"$COMPOSE_PROJECT_NAME\"" in workflow
    assert "if: always()" in workflow
    assert "--volumes --remove-orphans" in workflow
