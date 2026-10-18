"""Exercise image identity initialization with the actual Linux server binary.

Redirect all writable paths into a temporary directory; host/USB commands are
stubs. This is an initialization regression test, not a boot or USB acceptance.
"""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
source = (Path(__file__).resolve().parents[1] / 'scripts/images/firstboot.sh').read_text()
with tempfile.TemporaryDirectory(prefix='rust-usb-firstboot-') as temp:
    root = Path(temp)
    commands = root / 'bin'
    commands.mkdir()
    for name in ('hostnamectl', 'ssh-keygen', 'modprobe', 'findmnt'):
        stub = commands / name
        stub.write_text('#!/bin/sh\nexit 0\n')
        stub.chmod(0o755)
    env = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'])
    identities = []
    for number in (1, 2):
        instance = root / str(number)
        instance.mkdir()
        machine = instance / 'machine-id'
        machine.write_text(f'{number:032x}\n')
        script = source.replace('/var/lib/rust-usb-bridge', str(instance / 'state'))
        script = script.replace('/opt/rust-usb-bridge/usb-bridge-server', str(binary))
        script = script.replace('/opt/rust-usb-bridge/public', str(instance / 'public'))
        script = script.replace('/etc/machine-id', str(machine))
        path = instance / 'firstboot.sh'
        path.write_text(script)
        subprocess.run(['bash', str(path)], env=env, check=True)
        state = instance / 'state'
        assert (instance / 'state-image/initialized').is_file()
        before = {name: hashlib.sha256((state / name).read_bytes()).digest()
                  for name in ('config.json', 'ca.pem')}
        assert (instance / 'public/ca.pem').read_bytes() == (state / 'ca.pem').read_bytes()
        subprocess.run(['bash', str(path)], env=env, check=True)
        assert before == {name: hashlib.sha256((state / name).read_bytes()).digest()
                          for name in before}, 'Repeated startup replaced identity'
        identities.append(before['ca.pem'])
    assert identities[0] != identities[1], 'Fresh instances must not share a CA'
print('FIRSTBOOT_INITIALIZATION_OK: independent CAs; repeat startup preserves identity')
