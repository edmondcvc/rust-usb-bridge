#!/usr/bin/env python3
"""Audit selected public source files without reading deployment state."""
import pathlib,re,sys,subprocess
root=pathlib.Path(__file__).resolve().parent.parent
result=subprocess.run(['git','ls-files','-z'],cwd=root,capture_output=True)
if result.returncode: raise SystemExit('Run git init and git add before auditing')
errors=[]
for name in result.stdout.decode().split('\0'):
    if not name: continue
    p=pathlib.PurePosixPath(name)
    if any(part in {'.tools','dist','target','credentials','test-state','evidence','deployment-snapshots'} for part in p.parts): errors.append(name+': private/build directory')
    if p.suffix.lower() in {'.pem','.key','.pfx','.p12','.log','.img','.qcow2','.exe','.dll'} or p.name in {'client.json','config.json','pair-code.txt','installation.json','SYSTEM_INVENTORY.txt'}: errors.append(name+': machine state or binary')
    text=(root/name).read_bytes().decode('utf-8',errors='replace')
    if re.search(r'-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----',text): errors.append(name+': private key material')
    if re.search(r'192\.168\.1\.13|corel[-]usb|C:\\Users\\(?:39711|12862)',text): errors.append(name+': workstation-specific literal')
    if re.search(r'\b(?:ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})',text): errors.append(name+': access token')
print('\n'.join(errors) if errors else 'PUBLIC_SOURCE_AUDIT_OK')
sys.exit(bool(errors))
