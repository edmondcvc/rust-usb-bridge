#!/usr/bin/env python3
"""Portable Linux packages, source archives, checksums and 1-GiB image volumes."""
import argparse, hashlib, json, pathlib, shutil, tarfile, zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERSION = '0.2.0-rc.1'
def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for block in iter(lambda: f.read(1024*1024), b''): h.update(block)
    return h.hexdigest()
def package(binary, arch, output):
    data = binary.read_bytes()
    assert data[:5] == b'\x7fELF\x02' and data[5] == 1, 'Expected a little-endian ELF64 binary'
    assert int.from_bytes(data[18:20], 'little') == {'x86_64':62,'aarch64':183}[arch], 'ELF architecture mismatch'
    stage = output / ('stage-linux-' + arch)
    assert not stage.exists(), f'Refusing to overwrite stage: {stage}'
    stage.mkdir(parents=True)
    shutil.copy2(binary, stage/'usb-bridge-server')
    (stage/'usb-bridge-server').chmod(0o755)
    for item in ('LICENSE','THIRD_PARTY_NOTICES.md','Cargo.lock','README.md','README.zh-CN.md','README.html','README.zh-CN.html','CONTRIBUTING.md','CONTRIBUTING.html','SECURITY.md','SECURITY.html','CHANGELOG.md','CHANGELOG.html'):
        shutil.copy2(ROOT/item, stage/item)
    for item in ('docs','THIRD_PARTY_LICENSES'): shutil.copytree(ROOT/item,stage/item)
    shutil.copytree(ROOT/'scripts/linux',stage/'scripts/linux')
    for file in (stage/'scripts/linux').glob('*.sh'): file.chmod(0o755)
    (stage/'release.json').write_text(json.dumps({'version':VERSION,'arch':arch,'usb_hardware_acceptance':'pending','protocol_version':1})+'\n')
    sums(stage)
    archive=output/f'RustUSBBridge-Linux-{VERSION}-{arch}.tar.gz'
    assert not archive.exists(), 'Refusing to overwrite release archive'
    with tarfile.open(archive,'w:gz') as tar:
        for file in sorted(stage.rglob('*')):
            if file.is_file():
                relative=file.relative_to(stage)
                info=tar.gettarinfo(str(file),arcname=relative.as_posix())
                info.mode=0o755 if relative.as_posix()=='usb-bridge-server' or file.suffix=='.sh' else 0o644
                info.uid=info.gid=0; info.uname=info.gname='root'
                with file.open('rb') as stream: tar.addfile(info,stream)
    print(archive)
def sums(directory):
    files=sorted(p for p in directory.iterdir() if p.is_file() and p.name != 'SHA256SUMS')
    # Package stages need recursive checksums, while release assets use top-level checksums.
    if directory.name.startswith('stage-'): files=sorted(p for p in directory.rglob('*') if p.is_file() and p.name!='SHA256SUMS')
    (directory/'SHA256SUMS').write_text(''.join(f'{digest(p)}  {p.relative_to(directory).as_posix()}\n' for p in files),encoding='ascii')
def split(file, output):
    manifest={'file':file.name,'sha256':digest(file),'size':file.stat().st_size,'parts':[]}
    with file.open('rb') as source:
        number=1
        while True:
            block=source.read(1024**3)
            if not block: break
            part=output/f'{file.name}.part{number:03}'
            with part.open('xb') as dest: dest.write(block)
            manifest['parts'].append({'name':part.name,'sha256':digest(part),'size':len(block)})
            number+=1
    (output/(file.name+'.volumes.json')).write_text(json.dumps(manifest,indent=2)+'\n')
def merge(manifest_path, output):
    m=json.loads(manifest_path.read_text())
    assert pathlib.Path(m['file']).name == m['file'] and m['parts'], 'Invalid manifest'
    for part in m['parts']:
        assert pathlib.Path(part['name']).name == part['name'], 'Unsafe part name'
        path=manifest_path.parent/part['name']
        assert path.stat().st_size == part['size'] and digest(path)==part['sha256'], f'Bad part: {path.name}'
    dest=output/m['file']
    with dest.open('xb') as f:
        for part in m['parts']:
            with (manifest_path.parent/part['name']).open('rb') as source: shutil.copyfileobj(source,f)
    assert dest.stat().st_size==m['size'] and digest(dest)==m['sha256'], 'Merged image checksum mismatch'
    print(dest)
def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action',choices=['linux','checksums','split','merge','source'])
    parser.add_argument('--binary',type=pathlib.Path)
    parser.add_argument('--arch',choices=['x86_64','aarch64'])
    parser.add_argument('--file',type=pathlib.Path)
    parser.add_argument('--output',type=pathlib.Path,default=ROOT/'dist')
    args=parser.parse_args(); args.output.mkdir(parents=True,exist_ok=True)
    if args.action=='linux':
        if not args.binary or not args.arch: parser.error('linux requires --binary and --arch')
        package(args.binary,args.arch,args.output)
    elif args.action=='checksums': sums(args.output)
    elif args.action=='split': split(args.file,args.output)
    elif args.action=='merge': merge(args.file,args.output)
    else:
        import subprocess
        files=subprocess.check_output(['git','ls-files','-z'],cwd=ROOT).decode().split('\0')
        if not any(files): raise SystemExit('Initialize and stage the audited source first')
        archive=args.output/f'RustUSBBridge-Source-{VERSION}.zip'
        with zipfile.ZipFile(archive,'x',compression=zipfile.ZIP_DEFLATED) as z:
            for name in files:
                if name: z.write(ROOT/name,name)
        print(archive)
if __name__=='__main__': main()
