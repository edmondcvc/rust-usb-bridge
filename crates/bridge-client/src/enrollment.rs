//! Password bootstrap runs as the desktop user, never through the service or a log.
use crate::backend::ComputerInvitation;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{io::{BufRead, BufReader, Read, Write}, path::Path, process::{Command, Stdio}, sync::mpsc, time::{Duration, Instant}};
use std::os::windows::process::CommandExt;
use zeroize::Zeroize;

#[derive(Serialize)]
pub struct Input {
    pub force_release: Option<bridge_common::ForceReleaseRequest>,
    pub host: String,
    pub username: String,
    pub password: String,
    pub ssh_port: u16,
    pub port: u16,
}
impl Drop for Input { fn drop(&mut self) { self.password.zeroize(); } }
impl Input {
    pub fn validate(&self) -> Result<()> {
        if self.host.is_empty() || self.host.len() > 255 || !self.host.bytes().all(|c| c.is_ascii_alphanumeric() || b".:-".contains(&c)) || !self.host.as_bytes()[0].is_ascii_alphanumeric() { bail!("请填写主机的 IP 地址或域名，不要带 http://。"); }
        if self.username.is_empty() || !self.username.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-".contains(&c)) || self.username.starts_with('-') { bail!("请检查主机用户名。"); }
        if self.password.is_empty() || self.password.contains(['\r', '\n']) { bail!("请填写主机密码，密码不能包含换行。"); }
        if self.ssh_port == 0 || self.port == 0 { bail!("端口请填写 1–65535 的数字。"); }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(tag="event", rename_all="snake_case")]
pub enum Event {
    Released,
    Progress { message: String },
    Confirm { host: String, fingerprint: String },
    Complete { invitation: ComputerInvitation },
    Error { kind: String, message: String },
}
pub enum Action { Confirm(bool), Cancel }
pub struct Job { pub events: mpsc::Receiver<Event>, pub actions: mpsc::Sender<Action> }
impl Drop for Job { fn drop(&mut self) { let _ = self.actions.send(Action::Cancel); } }

pub fn start(script: &Path, input: Input) -> Result<Job> {
    input.validate()?;
    if !script.is_file() { bail!("连接工具不完整，请用新版安装包修复软件。"); }
    let mut child = Command::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script).arg("-PipeMode").creation_flags(0x08000000)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        .context("连接工具未能启动，请重新安装软件")?;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let (actions, action_rx) = mpsc::channel();
    let (lines_tx, lines_rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Bounded line reads: never accumulate unbounded output from a helper.
        let mut reader = BufReader::new(stdout);
        loop {
            let mut bytes = Vec::new();
            let result = std::io::Read::by_ref(&mut reader).take(40_001).read_until(b'\n', &mut bytes);
            match result {
                Ok(0) | Err(_) => break,
                Ok(_) if bytes.len() > 40_000 => break,
                Ok(_) => { if lines_tx.send(bytes).is_err() { break; } }
            }
        }
    });
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut line = serde_json::to_vec(&input)?;
            let sent = stdin.write_all(&line).and_then(|_| stdin.write_all(b"\n")).and_then(|_| stdin.flush());
            line.zeroize(); drop(input); sent?;
            let started = Instant::now();
            loop {
                match action_rx.try_recv() {
                    Ok(Action::Cancel) | Err(mpsc::TryRecvError::Disconnected) => bail!("已取消连接，可以修改信息后重试。"),
                    Ok(Action::Confirm(yes)) => { writeln!(stdin, "{}", if yes { "yes" } else { "no" })?; stdin.flush()?; },
                    _ => {}
                }
                if started.elapsed() > Duration::from_secs(180) { bail!("连接等待太久，请检查主机和网络后重试。"); }
                match lines_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(mut line) => {
                        let parsed = serde_json::from_slice::<Event>(&line);
                        line.zeroize();
                        let event = parsed.context("连接工具返回的信息不完整，请重新安装软件")?;
                        let done = matches!(event, Event::Complete {..} | Event::Error {..} | Event::Released);
                        if tx.send(event).is_err() { bail!("窗口已关闭"); }
                        if done { return Ok(()); }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => bail!("连接工具提前关闭，请重试；仍失败时请修复安装。"),
                    _ => {}
                }
            }
        })();
        if let Err(e) = result { let _ = tx.send(Event::Error { kind: "local".into(), message: e.to_string() }); }
        // Only this helper and its descendants: no global process-name termination.
        if child.try_wait().ok().flatten().is_none() {
            let _ = Command::new("taskkill.exe").args(["/PID", &child.id().to_string(), "/T", "/F"])
                .creation_flags(0x08000000).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        let _ = child.wait();
    });
    Ok(Job { events: rx, actions })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Input { Input { force_release: None, host:"127.0.0.1".into(), username:"operator".into(), password:"test-only-secret".into(),ssh_port:22,port:3241 } }
    fn script(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir=tempfile::tempdir().unwrap(); let path=dir.path().join("fixture.ps1");
        std::fs::write(&path,format!("\u{feff}{text}")).unwrap(); (dir,path)
    }
    #[test]
    fn helper_exchanges_password_via_stdin_and_confirmation_via_pipe() {
        let (_dir,path)=script(r#"param([switch]$PipeMode)
$r=[Console]::ReadLine() | ConvertFrom-Json
if($r.password -ne 'test-only-secret'){exit 2}
[Console]::WriteLine('{"event":"confirm","host":"127.0.0.1","fingerprint":"SHA256:example"}')
if([Console]::ReadLine() -ne 'yes'){exit 3}
[Console]::WriteLine('{"event":"error","kind":"password","message":"wrong password"}')
"#);
        let job=start(&path,input()).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(15)).unwrap(),Event::Confirm{..}));
        job.actions.send(Action::Confirm(true)).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(15)).unwrap(),Event::Error{kind,..} if kind=="password"));
    }
    #[test]
    fn cancellation_does_not_wait_for_helper_or_emit_success() {
        let (_dir,path)=script("param([switch]$PipeMode)\n$r=[Console]::ReadLine()\n[Console]::WriteLine('{\"event\":\"progress\",\"message\":\"working\"}')\nStart-Sleep -Seconds 60");
        let job=start(&path,input()).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(15)).unwrap(),Event::Progress{..}));
        job.actions.send(Action::Cancel).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(3)).unwrap(),Event::Error{..}));
    }
    #[test]
    fn truncated_helper_output_cannot_report_success() {
        let (_dir,path)=script("param([switch]$PipeMode)\n$r=[Console]::ReadLine()\n[Console]::WriteLine('not-json')");
        let job=start(&path,input()).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(15)).unwrap(),Event::Error{..}));
    }
    #[test]
    fn force_release_helper_uses_stdin_and_requires_explicit_completion() {
        let (_dir, path) = script(r#"param([switch]$PipeMode)
$r=[Console]::ReadLine() | ConvertFrom-Json
if($r.force_release.session_id -ne 'fixture-session' -or $r.password -ne 'test-only-secret'){exit 2}
[Console]::WriteLine('{"event":"released"}')
"#);
        let mut value = input();
        value.force_release = Some(bridge_common::ForceReleaseRequest {server_id:"fixture".into(),busid:"1-2".into(),session_id:"fixture-session".into(),path:"/fixture".into(),vendor:1,product:2,serial:None});
        let job = start(&path, value).unwrap();
        assert!(matches!(job.events.recv_timeout(Duration::from_secs(15)).unwrap(), Event::Released));
    }
    #[test]
    fn reject_options_urls_and_newlines_before_spawn() {
        let mut input = Input { force_release: None, host: "192.0.2.10".into(), username: "operator".into(), password: "example".into(), ssh_port: 22, port: 3241 };
        assert!(input.validate().is_ok());
        for host in ["-oProxyCommand=foo", "https://example.com", "host name", ""] { input.host=host.into(); assert!(input.validate().is_err()); }
        input.host="host.example".into(); input.password="a\nb".into(); assert!(input.validate().is_err());
    }
}
