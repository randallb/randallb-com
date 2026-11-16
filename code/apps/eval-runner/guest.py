"""Image-owned workload entry point. No inherited environment or input mounts."""
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys


def main():
    token, *argv = sys.argv[1:]
    directory = Path('/tmp') / ('rbdc-' + token)
    directory.mkdir(mode=0o700)
    record = {'token': token, 'argv': argv, 'system': list(os.uname()), 'kind': 'command'}
    with (directory / 'stdout').open('wb') as stdout, (directory / 'stderr').open('wb') as stderr:
        try:
            child = subprocess.Popen(argv, env={'PATH': '/bin', 'HOME': '/tmp', 'LANG': 'C.UTF-8'},
                                     cwd='/tmp', stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            streams = selectors.DefaultSelector()
            streams.register(child.stdout, selectors.EVENT_READ, (stdout, 1))
            streams.register(child.stderr, selectors.EVENT_READ, (stderr, 2))
            while streams.get_map():
                for key, _ in streams.select():
                    data = os.read(key.fd, 65536)
                    if not data:
                        streams.unregister(key.fileobj)
                        key.fileobj.close()
                    else:
                        target, fd = key.data
                        target.write(data)
                        target.flush()
                        os.write(fd, data)
            record['exit_status'] = child.wait()
        except OSError as error:
            record.update(kind='invalid_executable', exit_status=127, error=str(error))
            message = (str(error) + '\n').encode()
            stderr.write(message)
            os.write(2, message)
    (directory / 'result.json').write_text(json.dumps(record) + '\n')
    return record['exit_status'] if record['exit_status'] >= 0 else 128 - record['exit_status']


if __name__ == '__main__':
    sys.exit(main())
