from pathlib import Path

import yaml


for path in sorted(Path('.github/workflows').glob('*.yml')):
    yaml.safe_load(path.read_text())
    print(f'YAML OK: {path}')
