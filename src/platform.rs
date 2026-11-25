//! Windows 10/11 桌面集成：单实例、文件转交、系统托盘和“打开方式”注册。

#![cfg(windows)]

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexExW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const INSTANCE_NAME: &str = r"Global\EncryptoNote.SingleInstance";
const PIPE_NAME: &str = r"\\.\pipe\EncryptoNote.OpenFile";
const PROG_ID: &str = "EncryptoNote.ecnote";
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_PATHS: usize = 64;

pub enum Event {
    Open(Vec<PathBuf>),
    Show,
    Exit,
    Error(String),
}

#[derive(Clone)]
struct EventSender {
    tx: Sender<Event>,
    context: Arc<Mutex<Option<egui::Context>>>,
}

impl EventSender {
    fn send(&self, event: Event) -> Result<()> {
        self.tx
            .send(event)
            .map_err(|_| anyhow::anyhow!("应用正在退出"))?;
        if let Some(ctx) = self.context.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
        Ok(())
    }
}

pub struct Instance {
    events: Receiver<Event>,
    sender: EventSender,
    // 先停止服务，再释放整机互斥锁，避免退出期间产生两个主实例。
    _server: PipeServer,
    _guard: OwnedHandle,
}

impl Instance {
    /// 次实例转交参数后返回 None；通信或权限错误必须反馈，不能另开窗口。
    pub fn start() -> Result<Option<Self>> {
        let paths = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .filter(|path| has_note_extension(path))
            .map(std::path::absolute)
            .collect::<io::Result<Vec<_>>>()?;
        let mut guard = acquire_instance(INSTANCE_NAME)
            .context("无法检查全局实例；应用可能已在其他用户或权限环境中运行")?;
        if guard.is_none() {
            match send_request(PIPE_NAME, &paths) {
                Ok(()) => return Ok(None),
                Err(error) => {
                    // 前一个实例可能刚好退出，只在重新拿到全局锁时才启动。
                    guard = acquire_instance(INSTANCE_NAME)?;
                    if guard.is_none() {
                        return Err(error).context(
                            "无法联系已运行的 EncryptoNote；请检查其他登录会话或应用的运行权限",
                        );
                    }
                }
            }
        }
        let (tx, events) = mpsc::channel();
        let sender = EventSender {
            tx,
            context: Arc::new(Mutex::new(None)),
        };
        let server =
            PipeServer::start(PIPE_NAME, sender.clone()).context("无法启动文件转交服务")?;
        if !paths.is_empty() {
            sender.send(Event::Open(paths))?;
        }
        if let Err(error) = register_file_association() {
            sender.send(Event::Error(format!("注册“打开方式”失败：{error:#}")))?;
        }
        Ok(Some(Self {
            events,
            sender,
            _server: server,
            _guard: guard.unwrap(),
        }))
    }

    /// 托盘必须在 eframe 的 Windows 事件循环线程上创建。
    pub fn attach(self, ctx: &egui::Context) -> Desktop {
        *self.sender.context.lock().unwrap() = Some(ctx.clone());
        let tray = match create_tray(self.sender.clone()) {
            Ok(tray) => Some(tray),
            Err(error) => {
                let _ = self.sender.send(Event::Error(format!(
                    "系统托盘不可用，关闭窗口将退出应用：{error:#}"
                )));
                None
            }
        };
        Desktop {
            tray,
            instance: self,
        }
    }
}

pub struct Desktop {
    tray: Option<TrayIcon>,
    instance: Instance,
}

impl Desktop {
    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }

    pub fn drain_events(&self) -> Vec<Event> {
        self.instance.events.try_iter().collect()
    }
}

pub fn show_error(message: &str) {
    rfd::MessageDialog::new()
        .set_title("EncryptoNote")
        .set_level(rfd::MessageLevel::Error)
        .set_description(message)
        .show();
}

fn create_tray(sender: EventSender) -> Result<TrayIcon> {
    let menu = Menu::new();
    menu.append(&MenuItem::with_id("show", "显示主窗口", true, None))?;
    menu.append(&MenuItem::with_id("quit", "退出", true, None))?;
    let icon = crate::app_icon();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("EncryptoNote")
        .with_icon(Icon::from_rgba(icon.rgba, icon.width, icon.height)?)
        .build()?;
    let menu_sender = sender.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = match event.id().0.as_str() {
            "show" => Event::Show,
            "quit" => Event::Exit,
            _ => return,
        };
        let _ = menu_sender.send(action);
    }));
    TrayIconEvent::set_event_handler(Some(move |event| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        ) {
            let _ = sender.send(Event::Show);
        }
    }));
    Ok(tray)
}

fn has_note_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(crate::store::EXT))
}

fn acquire_instance(name: &str) -> io::Result<Option<OwnedHandle>> {
    // 只需持有命名对象，SYNCHRONIZE 避免打开已有锁时索取所有权限。
    let handle = unsafe { CreateMutexExW(null(), wide(name).as_ptr(), 0, 0x0010_0000) };
    let error = unsafe { GetLastError() };
    let guard = owned_handle(handle)?;
    Ok((error != ERROR_ALREADY_EXISTS).then_some(guard))
}

fn owned_handle(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // 只接收本模块 Windows API 新建的句柄，所有权转交后恰好关闭一次。
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn create_event() -> io::Result<OwnedHandle> {
    owned_handle(unsafe { CreateEventW(null(), 1, 0, null()) })
}

/// 每次操作的 OVERLAPPED、事件和缓冲区都存活到完成或取消完成。
struct PipeIo<'a> {
    handle: &'a OwnedHandle,
    stop: Option<&'a OwnedHandle>,
    deadline: Option<Instant>,
}

impl PipeIo<'_> {
    fn perform(&self, start: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<u32> {
        if self.stop.is_some_and(|stop| unsafe {
            WaitForSingleObject(stop.as_raw_handle(), 0) == WAIT_OBJECT_0
        }) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let timeout = match self.deadline {
            Some(deadline) => {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or(io::ErrorKind::TimedOut)?;
                remaining
                    .as_millis()
                    .saturating_add(1)
                    .min(u32::MAX as u128 - 1) as u32
            }
            None => INFINITE,
        };
        let event = create_event()?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.as_raw_handle(),
            ..unsafe { std::mem::zeroed() }
        };
        let handle = self.handle.as_raw_handle();
        if start(&mut overlapped) == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_PIPE_CONNECTED {
                return Ok(0);
            }
            if error != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            let mut handles = vec![event.as_raw_handle()];
            if let Some(stop) = self.stop {
                handles.insert(0, stop.as_raw_handle());
            }
            let waited = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout)
            };
            if waited != WAIT_OBJECT_0 + handles.len() as u32 - 1 {
                let error = match waited {
                    WAIT_TIMEOUT => io::ErrorKind::TimedOut.into(),
                    WAIT_FAILED => io::Error::last_os_error(),
                    _ => io::ErrorKind::Interrupted.into(),
                };
                // CancelIoEx 只是请求取消；必须等待完成后才能释放栈上的结构与缓冲区。
                unsafe {
                    CancelIoEx(handle, &overlapped);
                    let mut transferred = 0;
                    GetOverlappedResult(handle, &overlapped, &mut transferred, 1);
                }
                return Err(error);
            }
        }
        let mut transferred = 0;
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(transferred)
    }

    fn connect(&self) -> io::Result<()> {
        self.perform(|overlapped| unsafe {
            ConnectNamedPipe(self.handle.as_raw_handle(), overlapped)
        })
        .map(|_| ())
    }

    fn read_exact(&self, mut buffer: &mut [u8]) -> io::Result<()> {
        while !buffer.is_empty() {
            let read = self.perform(|overlapped| unsafe {
                ReadFile(
                    self.handle.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    null_mut(),
                    overlapped,
                )
            })? as usize;
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            buffer = &mut buffer[read..];
        }
        Ok(())
    }

    fn write_all(&self, mut buffer: &[u8]) -> io::Result<()> {
        while !buffer.is_empty() {
            let written = self.perform(|overlapped| unsafe {
                WriteFile(
                    self.handle.as_raw_handle(),
                    buffer.as_ptr(),
                    buffer.len() as u32,
                    null_mut(),
                    overlapped,
                )
            })? as usize;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            buffer = &buffer[written..];
        }
        Ok(())
    }
}

struct PipeServer {
    stop: Arc<OwnedHandle>,
    worker: Option<JoinHandle<()>>,
}

impl PipeServer {
    fn start(name: &str, sender: EventSender) -> Result<Self> {
        // 同步确认监听成功；始终复用这一句柄，避免重建间隙被其他进程占用。
        let pipe = owned_handle(unsafe {
            CreateNamedPipeW(
                wide(name).as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                4096,
                0,
                null(),
            )
        })?;
        let stop = Arc::new(create_event()?);
        let thread_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("ecnote-open-file".into())
            .spawn(move || {
                loop {
                    let connection = PipeIo {
                        handle: &pipe,
                        stop: Some(&thread_stop),
                        deadline: None,
                    };
                    match connection.connect() {
                        Ok(()) => {
                            let io = PipeIo {
                                deadline: Some(Instant::now() + IO_TIMEOUT),
                                ..connection
                            };
                            let _ = (|| -> Result<()> {
                                let mut length = [0; 4];
                                io.read_exact(&mut length)?;
                                let length = u32::from_le_bytes(length) as usize;
                                ensure!(length <= MAX_REQUEST_BYTES, "打开请求过大");
                                let mut payload = vec![0; length];
                                io.read_exact(&mut payload)?;
                                let paths = decode_request(&payload)?;
                                sender.send(Event::Open(paths))?;
                                io.write_all(&[1])?;
                                // 等客户端确认读到 ACK 后再断开，避免丢弃尚未读取的 ACK。
                                let mut receipt = [0];
                                io.read_exact(&mut receipt)?;
                                Ok(())
                            })();
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => break,
                        Err(error) if error.raw_os_error() == Some(ERROR_NO_DATA as i32) => {}
                        Err(error) => {
                            let _ =
                                sender.send(Event::Error(format!("文件转交服务已停止：{error}")));
                            break;
                        }
                    }
                    unsafe {
                        DisconnectNamedPipe(pipe.as_raw_handle());
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Serialize, Deserialize)]
struct OpenRequest {
    // Windows 文件名允许非 Unicode UTF-16；不能经 to_string_lossy 损坏路径。
    paths: Vec<Vec<u16>>,
}

fn encode_request(paths: &[PathBuf]) -> Result<Vec<u8>> {
    ensure!(paths.len() <= MAX_PATHS, "一次最多打开 {MAX_PATHS} 个文件");
    let paths = paths
        .iter()
        .map(|path| {
            Ok(std::path::absolute(path)?
                .as_os_str()
                .encode_wide()
                .collect())
        })
        .collect::<io::Result<Vec<Vec<u16>>>>()?;
    let payload = serde_json::to_vec(&OpenRequest { paths })?;
    ensure!(payload.len() <= MAX_REQUEST_BYTES, "打开请求过大");
    Ok(payload)
}

fn decode_request(payload: &[u8]) -> Result<Vec<PathBuf>> {
    ensure!(payload.len() <= MAX_REQUEST_BYTES, "打开请求过大");
    let request: OpenRequest = serde_json::from_slice(payload)?;
    ensure!(request.paths.len() <= MAX_PATHS, "打开文件数过多");
    request
        .paths
        .into_iter()
        .map(|path| {
            ensure!(!path.contains(&0), "文件路径包含空字符");
            let path = PathBuf::from(OsString::from_wide(&path));
            ensure!(
                path.is_absolute() && has_note_extension(&path),
                "无效的笔记路径"
            );
            Ok(path)
        })
        .collect()
}

fn send_request(name: &str, paths: &[PathBuf]) -> Result<()> {
    let payload = encode_request(paths)?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let pipe: OwnedHandle = loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name)
        {
            Ok(pipe) => break pipe.into(),
            Err(error) => {
                // 只重试尚未就绪/忙碌的管道，权限错误直接向用户反馈。
                if !matches!(error.raw_os_error(), Some(2 | 231)) || Instant::now() >= deadline {
                    return Err(error.into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    };
    // 把新启动进程的前台权限交给主实例，便于从资源管理器唤回窗口。
    unsafe {
        let mut process_id = 0;
        if GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut process_id) != 0 {
            AllowSetForegroundWindow(process_id);
        }
    }
    let io = PipeIo {
        handle: &pipe,
        stop: None,
        deadline: Some(deadline),
    };
    io.write_all(&(payload.len() as u32).to_le_bytes())?;
    io.write_all(&payload)?;
    let mut ack = [0];
    io.read_exact(&mut ack)?;
    ensure!(ack == [1], "已有实例未确认打开请求");
    // ACK 已证明主实例接收；收尾失败不重复提交同一个请求。
    let _ = io.write_all(&[1]);
    Ok(())
}

fn register_file_association() -> Result<()> {
    let executable = std::env::current_exe()?;
    let (classes, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(r"Software\Classes")?;
    register_association_at(&classes, &executable)?;
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null());
    }
    Ok(())
}

fn register_association_at(classes: &RegKey, executable: &Path) -> Result<()> {
    let mut command = OsString::from("\"");
    command.push(executable);
    let mut icon = command.clone();
    command.push("\" \"%1\"");
    icon.push("\",0");
    let (open_with, _) = classes.create_subkey(r".ecnote\OpenWithProgids")?;
    open_with.set_value(PROG_ID, &"")?;
    let (progid, _) = classes.create_subkey(PROG_ID)?;
    progid.set_value("", &"EncryptoNote 加密笔记")?;
    progid
        .create_subkey("DefaultIcon")?
        .0
        .set_value("", &icon)?;
    progid
        .create_subkey(r"shell\open\command")?
        .0
        .set_value("", &command)?;
    let (application, _) = progid.create_subkey("Application")?;
    application.set_value("ApplicationName", &"EncryptoNote")?;
    application.set_value("ApplicationDescription", &"EncryptoNote 加密富文本笔记")?;
    application.set_value("ApplicationIcon", &icon)?;

    let name = executable.file_name().context("无法取得应用文件名")?;
    let mut application_key = OsString::from("Applications\\");
    application_key.push(name);
    let (application, _) = classes.create_subkey(application_key)?;
    application.set_value("FriendlyAppName", &"EncryptoNote")?;
    application
        .create_subkey("SupportedTypes")?
        .0
        .set_value(".ecnote", &"")?;
    application
        .create_subkey("DefaultIcon")?
        .0
        .set_value("", &icon)?;
    application
        .create_subkey(r"shell\open\command")?
        .0
        .set_value("", &command)?;
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_name(prefix: &str) -> String {
        format!("{prefix}-{}", crate::recent::random_id())
    }

    fn test_sender() -> (EventSender, Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        (
            EventSender {
                tx,
                context: Arc::new(Mutex::new(None)),
            },
            rx,
        )
    }

    #[test]
    fn global_mutex_is_exclusive_and_released() {
        let name = unique_name(r"Global\EncryptoNote.Test");
        let first = acquire_instance(&name).unwrap().unwrap();
        assert!(acquire_instance(&name).unwrap().is_none());
        drop(first);
        assert!(acquire_instance(&name).unwrap().is_some());
    }

    #[test]
    fn request_preserves_windows_paths_and_resolves_relative_paths() {
        let mut unusual: Vec<u16> = r"C:\含 空格\".encode_utf16().collect();
        unusual.push(0xD800);
        unusual.extend(".ECNOTE".encode_utf16());
        let paths = vec![
            PathBuf::from("相对路径.ecnote"),
            PathBuf::from(OsString::from_wide(&unusual)),
        ];
        let decoded = decode_request(&encode_request(&paths).unwrap()).unwrap();
        assert_eq!(decoded[0], std::path::absolute(&paths[0]).unwrap());
        assert_eq!(decoded[1], paths[1]);
        assert!(decode_request(br#"{"paths":[[0]]}"#).is_err());
        assert!(decode_request(&vec![0; MAX_REQUEST_BYTES + 1]).is_err());
        assert!(encode_request(&vec![paths[0].clone(); MAX_PATHS + 1]).is_err());
    }

    #[test]
    fn pipe_forwards_multiple_requests_and_can_stop_while_idle() {
        let name = unique_name(r"\\.\pipe\EncryptoNote.Test");
        let (sender, events) = test_sender();
        let server = PipeServer::start(&name, sender).unwrap();
        for paths in [vec![PathBuf::from(r"C:\笔记 文件.ECNOTE")], Vec::new()] {
            send_request(&name, &paths).unwrap();
            match events.recv_timeout(IO_TIMEOUT).unwrap() {
                Event::Open(received) => assert_eq!(received, paths),
                _ => panic!("错误的桌面事件"),
            }
        }
        drop(server);
        assert!(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
                .is_err()
        );
    }

    #[test]
    fn pipe_rejects_oversize_messages_and_stop_cancels_a_stalled_client() {
        let name = unique_name(r"\\.\pipe\EncryptoNote.Test");
        let (sender, events) = test_sender();
        let server = PipeServer::start(&name, sender).unwrap();
        {
            use std::io::Write;
            let mut client = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&name)
                .unwrap();
            client
                .write_all(&((MAX_REQUEST_BYTES + 1) as u32).to_le_bytes())
                .unwrap();
        }
        send_request(&name, &[]).unwrap();
        assert!(matches!(
            events.recv_timeout(IO_TIMEOUT).unwrap(),
            Event::Open(_)
        ));
        let deadline = Instant::now() + IO_TIMEOUT;
        let _stalled = loop {
            match OpenOptions::new().read(true).write(true).open(&name) {
                Ok(client) => break client,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("{error}"),
            }
        };
        let start = Instant::now();
        drop(server);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn association_uses_friendly_name_quoted_paths_and_preserves_default() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let name = unique_name(r"Software\EncryptoNote\Tests\Association");
        let (classes, _) = hkcu.create_subkey(&name).unwrap();
        let executable = Path::new(r"C:\应用 程序\EncryptoNote.exe");
        classes
            .create_subkey(".ecnote")
            .unwrap()
            .0
            .set_value("", &"Another.App")
            .unwrap();
        register_association_at(&classes, executable).unwrap();
        let default: String = classes
            .open_subkey(".ecnote")
            .unwrap()
            .get_value("")
            .unwrap();
        let command: String = classes
            .open_subkey(r"EncryptoNote.ecnote\shell\open\command")
            .unwrap()
            .get_value("")
            .unwrap();
        let app_name: String = classes
            .open_subkey(r"Applications\EncryptoNote.exe")
            .unwrap()
            .get_value("FriendlyAppName")
            .unwrap();
        let supported: String = classes
            .open_subkey(r"Applications\EncryptoNote.exe\SupportedTypes")
            .unwrap()
            .get_value(".ecnote")
            .unwrap();
        drop(classes);
        hkcu.delete_subkey_all(&name).unwrap();
        assert_eq!(default, "Another.App");
        assert_eq!(command, "\"C:\\应用 程序\\EncryptoNote.exe\" \"%1\"");
        assert_eq!(app_name, "EncryptoNote");
        assert_eq!(supported, "");
    }
}
