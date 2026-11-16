"""One-way copy of existing host Codex auth into a disposable guest."""
import json
import os
from pathlib import Path

GUEST_HOME = '/private/tmp/rbdc-codex-home'


def source(home=None):
    directory = Path(home or os.environ.get('CODEX_HOME') or Path.home() / '.codex').expanduser()
    path = directory / 'auth.json'
    try:
        data = json.loads(path.read_bytes())
        if not isinstance(data, dict):
            raise ValueError()
        tokens = data.get('tokens')
        if not (data.get('OPENAI_API_KEY') or (isinstance(tokens, dict) and tokens.get('access_token'))):
            raise ValueError()
    except (OSError, ValueError):
        raise RuntimeError('Existing Codex auth.json is missing or unsupported; no sign-in was started') from None
    return path


def install(machine, path):
    # Credentials travel only on stdin; never stdout, argv, images or host mounts.
    script = 'set -eu; umask 077; test ! -e "$1"; /bin/mkdir "$1"; /bin/cat > "$1/auth.json"; /bin/chmod 600 "$1/auth.json"; printf \'%s\\n\' \'cli_auth_credentials_store = "file"\' > "$1/config.toml"'
    machine.call(['exec', '-i', machine.record['machine'], '/bin/sh', '-c',
                  script, 'rbdc', GUEST_HOME], input_path=path)
    return {'mode': 'existing-host-auth-copy', 'guest_home': GUEST_HOME,
            'host_writeback': False, 'credential_lifetime': 'disposable-guest'}
