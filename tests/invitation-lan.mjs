// Real Windows-to-Linux TLS; only a separate mock server on port 3243.
import tls from 'node:tls';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import fs from 'node:fs/promises';
const exec = promisify(execFile);
const remote = '/home/operator/rust-usb-invitation-test-20261011';
const sshArgs = ['-F','NUL','-T','-i','C:/Users/39711/AppData/Local/CorelUsbBridge/ssh/id_ed25519','-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile=C:/Users/39711/AppData/Local/CorelUsbBridge/ssh/known_hosts','operator@192.0.2.10'];
function assert(value, label) { if (!value) throw new Error(label); }
export async function run(output, prepared) {
  const ca = prepared ? await fs.readFile(prepared+'/ca.pem','utf8') : (await exec('ssh.exe',[...sshArgs,`cat '${remote}/state/ca.pem'`])).stdout;
  const code = prepared ? (await fs.readFile(prepared+'/bootstrap.txt','utf8')).trim() : (await exec('ssh.exe',[...sshArgs,`'${remote}/server' pair-code --mock --state-dir '${remote}/state'`])).stdout.trim();
  const folder = output.replace(/[^/\\]+$/,'') + 'invitation-lan-private';
  await fs.mkdir(folder,{recursive:true});
  const report = {kind:'Windows to real Linux LAN, isolated simulated USB',physical_usb_tested:false,checks:[],ok:false};
  const sockets = [];
  function passed(label) { report.checks.push(label); }
  async function connection(bundle) {
    const socket = tls.connect({host:'192.0.2.10',port:3243,servername:'usb-bridge',ca,cert:bundle?.client_cert_pem,key:bundle?.client_key_pem,rejectUnauthorized:true});
    sockets.push(socket); socket.setTimeout(120000,()=>socket.destroy(new Error('management timeout')));
    await new Promise((resolve,reject)=>{socket.once('secureConnect',resolve);socket.once('error',reject);});
    return socket;
  }
  async function control(request,bundle) {
    const socket = await connection(bundle);
    const data=Buffer.from(JSON.stringify(request)); const length=Buffer.alloc(4);length.writeUInt32BE(data.length);
    return await new Promise((resolve,reject)=>{
      let buffer=Buffer.alloc(0);
      socket.on('error',reject);
      socket.on('data',chunk=>{
        buffer=Buffer.concat([buffer,chunk]);
        if(buffer.length>=4){const size=buffer.readUInt32BE();if(size>1048576){reject(new Error('oversize response'));socket.destroy();}
          else if(buffer.length>=size+4){resolve(JSON.parse(buffer.subarray(4,size+4).toString()));socket.destroy();}}
      });
      socket.write(Buffer.concat([Buffer.from('control\n'),length,data]));
    });
  }
  async function usb(bundle) {
    const socket=await connection(bundle);let buffer=Buffer.alloc(0);let waiting;
    socket.on('data',chunk=>{buffer=Buffer.concat([buffer,chunk]);waiting?.();});
    const read=size=>new Promise((resolve,reject)=>{const check=()=>{if(buffer.length>=size){const bytes=buffer.subarray(0,size);buffer=buffer.subarray(size);waiting=null;resolve(bytes);}};waiting=check;socket.once('error',reject);check();});
    const head=Buffer.from([1,17,128,3,0,0,0,0]);const busid=Buffer.alloc(32);busid.write('1-2');
    socket.write(Buffer.concat([Buffer.from('usbip\n'),head,busid]));const reply=await read(8);
    const ok=reply.readUInt32BE(4)===0;if(ok)await read(312);
    return {socket,ok,read};
  }
  try {
    assert(!(await control({op:'create_invitation'})).ok,'unpaired invitation accepted');passed('Unpaired computer cannot create invitation');
    const a=await control({op:'pair',code,name:'Invitation LAN PC A'});assert(a.ok&&a.pairing,'bootstrap failed');
    const invite=await control({op:'create_invitation'},a.pairing);assert(invite.ok&&invite.invitation,'invitation failed');passed('Enrolled PC A creates invitation over mTLS');
    const b=await control({op:'pair',code:invite.invitation.code,name:'Invitation LAN PC B'});assert(b.ok&&b.pairing,'invited enrollment failed');assert(b.pairing.client_id!==a.pairing.client_id,'identities reused');passed('PC B enrolls with independent identity without Linux pair-code');
    assert(!(await control({op:'pair',code:invite.invitation.code,name:'Replay'})).ok,'invitation replay accepted');passed('Invitation replay rejected');
    let list=await control({op:'list_devices'},a.pairing);assert(list.ok&&list.devices.every(d=>d.vendor===0xf000),'not mock devices');
    assert((await control({op:'set_shared',busid:'1-2',shared:true},a.pairing)).ok,'mock bind failed');
    const first=await usb(a.pairing);assert(first.ok,'PC A import failed');
    const blocked=await usb(b.pairing);assert(!blocked.ok,'PC B stole occupied device');blocked.socket.destroy();passed('Occupied device rejects concurrent PC B');
    first.socket.destroy();
    for(let attempt=0;attempt<30;attempt++){list=await control({op:'list_devices'},b.pairing);if(!list.devices.find(d=>d.busid==='1-2').owner)break;await new Promise(r=>setTimeout(r,100));}
    assert(!list.devices.find(d=>d.busid==='1-2').owner,'lease not released');
    const second=await usb(b.pairing);assert(second.ok,'PC B handoff failed');
    const payload=Buffer.from('handoff confirmed by simulated byte echo');second.socket.write(payload);assert((await second.read(payload.length)).equals(payload),'byte echo mismatch');passed('After PC A releases, PC B connects and exchanges bytes');
    second.socket.destroy();report.ok=true;
  } finally {
    for(const socket of sockets)socket.destroy();
    await fs.writeFile(output,JSON.stringify(report,null,2));
  }
  return report;
}
