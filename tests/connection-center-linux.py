#!/usr/bin/env python3
"""Isolated release-binary integration: real TLS/USBIP echo, no hardware."""
import json, os, pathlib, socket, ssl, struct, subprocess, sys, tempfile, time

binary = sys.argv[1]
def exact(s, n):
    data = b''
    while len(data) < n:
        part = s.recv(n-len(data))
        if not part: raise EOFError()
        data += part
    return data

with tempfile.TemporaryDirectory(prefix='usb-center-') as tmp:
    root = pathlib.Path(tmp); state = root/'state'; sysfs = root/'sys'; proc = root/'proc'
    (proc/'self').mkdir(parents=True); (proc/'self/mountinfo').write_text('')
    for busid, cls in [('1-2','0e'), ('1-3','08')]:
        d = sysfs/'bus/usb/devices'/busid; d.mkdir(parents=True)
        for key, value in dict(idVendor='1234',idProduct='5678',bDeviceClass=cls,busnum='1',devnum=busid[-1],usbip_status='1',**{'mock-driver':'usbip-host'}).items():
            (d/key).write_text(value)
    subprocess.run([binary,'init','--mock','--state-dir',str(state),'--server-name','usb-bridge'],check=True,stdout=subprocess.DEVNULL)
    conf = json.loads((state/'config.json').read_text())
    for busid in ['1-2','1-3']:
        conf['shared'][busid] = dict(vendor=0x1234,product=0x5678,serial=None,path=str(sysfs/'bus/usb/devices'/busid),original_driver='usbip-host')
    (state/'config.json').write_text(json.dumps(conf))
    with socket.socket() as slot: slot.bind(('127.0.0.1',0)); port=slot.getsockname()[1]
    log=open(root/'server.log','wb')
    server=subprocess.Popen([binary,'run','--mock','--state-dir',str(state),'--sysfs-root',str(sysfs),'--proc-root',str(proc),'--listen',f'127.0.0.1:{port}'],stdout=log,stderr=log)
    streams=[]
    try:
        for _ in range(100):
            if (state/'admin.sock').exists():
                try:
                    socket.create_connection(('127.0.0.1',port),timeout=.2).close(); break
                except OSError: pass
            time.sleep(.05)
        else: raise RuntimeError('isolated server did not start')
        def context(): return ssl.create_default_context(cafile=str(state/'ca.pem'))
        def connect(ctx): return ctx.wrap_socket(socket.create_connection(('127.0.0.1',port),timeout=5),server_hostname='usb-bridge')
        def control(ctx, request):
            with connect(ctx) as s:
                data=json.dumps(request).encode(); s.sendall(b'control\n'+struct.pack('!I',len(data))+data)
                response=json.loads(exact(s,struct.unpack('!I',exact(s,4))[0]))
                assert response['ok'], response.get('error')
                return response
        clients=[]
        for i in range(2):
            code=json.loads((state/'config.json').read_text())['pair_code']
            bundle=control(context(),dict(op='pair',code=code,name=f'Fixture PC {i}'))['pairing']
            crt=root/f'client{i}.pem'; key=root/f'client{i}.key'
            crt.write_text(bundle['client_cert_pem']); key.write_text(bundle['client_key_pem']); key.chmod(0o600)
            ctx=context(); ctx.load_cert_chain(str(crt),str(key)); clients.append(ctx)
        for ctx,busid in zip(clients,['1-2','1-3']):
            s=connect(ctx); s.sendall(b'usbip\n'+struct.pack('!HHI',0x111,0x8003,0)+busid.encode().ljust(32,b'\0'))
            assert struct.unpack('!HHI',exact(s,8))==(0x111,3,0); exact(s,312); streams.append(s)
        listing=control(clients[0],dict(op='list_devices'))
        a=next(d for d in listing['devices'] if d['busid']=='1-2')
        assert a['device_type']=='摄像头' and a['owner']=='Fixture PC 0'
        assert next(d for d in listing['devices'] if d['busid']=='1-3')['device_type']=='存储设备'
        request={k:a[k] for k in ['busid','session_id','path','vendor','product','serial']}; request['server_id']=listing['server_id']
        def release(req):
            return subprocess.run([binary,'force-release','--state-dir',str(state)],input=json.dumps(req).encode(),stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=25)
        wrong=dict(request,session_id='old-generation'); assert release(wrong).returncode != 0
        for s in streams: s.sendall(b'before'); assert exact(s,6)==b'before'
        # Ordinary Linux user cannot reach the root-only management socket.
        root.chmod(0o755); state.chmod(0o755)
        denied=subprocess.run([sys.executable,'-c',"import socket,sys;s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1])",str(state/'admin.sock')],preexec_fn=lambda:os.setuid(65534),stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        assert denied.returncode != 0 and b'PermissionError' in denied.stderr
        state.chmod(0o700)
        done=release(request); assert done.returncode==0,done.stderr.decode()
        assert json.loads(done.stdout)['ok']
        assert streams[0].recv(1)==b''
        streams[1].sendall(b'other-device-still-running'); assert exact(streams[1],26)==b'other-device-still-running'
        after=control(clients[1],dict(op='list_devices'))
        assert next(d for d in after['devices'] if d['busid']=='1-2')['owner'] is None
        assert next(d for d in after['devices'] if d['busid']=='1-3')['owner']=='Fixture PC 1'
        assert release(request).returncode != 0
        print(json.dumps(dict(ok=True,scope='isolated real Linux TLS + simulated USB bytes',checks=['two independent clients','device classes','stale session rejected','non-root admin denied','targeted force release confirmed','other device echo preserved','old request replay rejected']),ensure_ascii=False))
    finally:
        for s in streams: s.close()
        server.terminate()
        try: server.wait(timeout=10)
        except subprocess.TimeoutExpired: server.kill(); server.wait()
        log.close()
