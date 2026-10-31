//! Native ConPTY input regressions, run by the Windows CI test matrix.
//! Run with `cargo test -p uncurses --test windows_conpty -- --nocapture`.

#![cfg(windows)]

use std::ffi::c_void;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use uncurses::event::{Event, KeyCode, KeyModifiers};
use uncurses::program::{Program, ProgramOptions};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleCP,
    GetConsoleMode, GetStdHandle, HPCON, STD_INPUT_HANDLE, SetConsoleCP, SetConsoleMode,
    SetConsoleOutputCP,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
    PROCESS_INFORMATION, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

const TIMEOUT: Duration = Duration::from_secs(20);
const INPUT: &[u8] = b"caf\xc3\xa9 \xf0\x9f\x9a\x80 x";

struct PseudoConsole(HPCON);

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        unsafe { ClosePseudoConsole(self.0) };
    }
}

struct AttributeList(Vec<usize>);

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast()) };
    }
}

fn wait_for_output(output: &Receiver<Vec<u8>>, transcript: &mut Vec<u8>, marker: &[u8]) {
    let deadline = Instant::now() + TIMEOUT;
    while !transcript.windows(marker.len()).any(|part| part == marker) {
        let chunk = output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|err| {
                panic!(
                    "wait for {}: {err}\nConPTY output:\n{}",
                    String::from_utf8_lossy(marker),
                    String::from_utf8_lossy(transcript)
                )
            });
        transcript.extend_from_slice(&chunk);
    }
}

fn run_conpty(child_test: &str) {
    let (input_read, mut input_write) = io::pipe().unwrap();
    let (mut output_read, output_write) = io::pipe().unwrap();
    let mut console = 0;
    let result = unsafe {
        CreatePseudoConsole(
            COORD { X: 120, Y: 36 },
            input_read.as_raw_handle(),
            output_write.as_raw_handle(),
            0,
            &mut console,
        )
    };
    assert_eq!(result, 0, "CreatePseudoConsole: HRESULT {result:#x}");
    let console = PseudoConsole(console);

    let (sender, output) = mpsc::channel();
    let reader = std::thread::spawn(move || -> io::Result<()> {
        let mut buf = [0; 4096];
        loop {
            let count = output_read.read(&mut buf)?;
            if count == 0 {
                return Ok(());
            }
            // Keep the pipe drained during cleanup even if the receiver closes.
            let _ = sender.send(buf[..count].to_vec());
        }
    });

    let mut bytes = 0;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes) };
    assert_ne!(
        bytes,
        0,
        "attribute list size: {}",
        io::Error::last_os_error()
    );
    let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
    assert_ne!(
        unsafe { InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes) },
        0,
        "InitializeProcThreadAttributeList: {}",
        io::Error::last_os_error()
    );
    let mut attributes = AttributeList(storage);
    assert_ne!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes.0.as_mut_ptr().cast(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console.0 as *const c_void,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            )
        },
        0,
        "UpdateProcThreadAttribute: {}",
        io::Error::last_os_error()
    );

    let exe = std::env::current_exe().unwrap();
    let application: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let mut command: Vec<u16> = format!(
        "\"{}\" --exact {child_test} --ignored --nocapture --test-threads=1",
        exe.display()
    )
    .encode_utf16()
    .chain([0])
    .collect();
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT,
                null(),
                null(),
                &startup.StartupInfo,
                &mut info,
            )
        },
        0,
        "CreateProcessW: {}",
        io::Error::last_os_error()
    );
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
    drop(attributes);
    drop(input_read);
    drop(output_write);

    let mut transcript = Vec::new();
    for marker in [b"CONPTY_READY_INIT".as_slice(), b"CONPTY_READY_RESUME"] {
        wait_for_output(&output, &mut transcript, marker);
        input_write.write_all(INPUT).unwrap();
    }
    wait_for_output(&output, &mut transcript, b"CONPTY_INPUT_PASS");
    assert_eq!(
        unsafe { WaitForSingleObject(process.as_raw_handle(), TIMEOUT.as_millis() as u32) },
        WAIT_OBJECT_0,
        "child did not exit\n{}",
        String::from_utf8_lossy(&transcript)
    );
    let mut exit_code = 0;
    assert_ne!(
        unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut exit_code) },
        0,
        "GetExitCodeProcess: {}",
        io::Error::last_os_error()
    );
    assert_eq!(
        exit_code,
        0,
        "child exit code\n{}",
        String::from_utf8_lossy(&transcript)
    );
    drop(input_write);
    drop(console);
    reader.join().unwrap().unwrap();
}

fn input_mode() -> u32 {
    let mut mode = 0;
    assert_ne!(
        unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) },
        0,
        "GetConsoleMode: {}",
        io::Error::last_os_error()
    );
    mode
}

fn check_input(program: &mut Program<uncurses::terminal::TtyInput, uncurses::terminal::TtyOutput>) {
    for code in [
        KeyCode::Char('c'),
        KeyCode::Char('a'),
        KeyCode::Char('f'),
        KeyCode::Char('\u{e9}'),
        KeyCode::Space,
        KeyCode::Char('\u{1f680}'),
        KeyCode::Space,
        KeyCode::Char('x'),
    ] {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            assert!(
                program
                    .poll_event(Some(deadline.saturating_duration_since(Instant::now())))
                    .unwrap(),
                "missing key {code:?}"
            );
            match program.read_event().unwrap() {
                Event::Resize(_) => continue,
                Event::KeyPress(key) => {
                    assert_eq!(key.code, code);
                    assert_eq!(key.modifiers, KeyModifiers::empty());
                    if code == KeyCode::Char('\u{1f680}') {
                        assert_eq!(key.text.as_deref(), Some("\u{1f680}"));
                    }
                }
                event => panic!("expected key {code:?}, got {event:?}"),
            }
            break;
        }
    }
}

fn run_child(code_page: u32) {
    assert_ne!(
        unsafe { SetConsoleCP(code_page) },
        0,
        "SetConsoleCP: {}",
        io::Error::last_os_error()
    );
    assert_ne!(
        unsafe { SetConsoleOutputCP(65001) },
        0,
        "SetConsoleOutputCP: {}",
        io::Error::last_os_error()
    );
    assert_ne!(
        unsafe {
            SetConsoleMode(
                GetStdHandle(STD_INPUT_HANDLE),
                input_mode() & !ENABLE_VIRTUAL_TERMINAL_INPUT,
            )
        },
        0,
        "SetConsoleMode: {}",
        io::Error::last_os_error()
    );
    let mut program = Program::open().unwrap();
    assert_eq!(input_mode() & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);
    program.init_with(ProgramOptions::default()).unwrap();

    for marker in ["CONPTY_READY_INIT", "CONPTY_READY_RESUME"] {
        assert_eq!(unsafe { GetConsoleCP() }, code_page);
        assert_ne!(input_mode() & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);
        println!("{marker}");
        io::stdout().flush().unwrap();
        check_input(&mut program);
        if marker == "CONPTY_READY_INIT" {
            program.pause().unwrap();
            assert_eq!(input_mode() & ENABLE_VIRTUAL_TERMINAL_INPUT, 0);
            program.resume().unwrap();
        }
    }
    program.finish().unwrap();
    println!("CONPTY_INPUT_PASS");
    io::stdout().flush().unwrap();
}

#[test]
fn non_bmp_input_cp437() {
    run_conpty("child_cp437");
}

#[test]
fn non_bmp_input_cp65001() {
    run_conpty("child_cp65001");
}

#[test]
#[ignore = "fixture launched inside a ConPTY by non_bmp_input_cp437"]
fn child_cp437() {
    run_child(437);
}

#[test]
#[ignore = "fixture launched inside a ConPTY by non_bmp_input_cp65001"]
fn child_cp65001() {
    run_child(65001);
}
