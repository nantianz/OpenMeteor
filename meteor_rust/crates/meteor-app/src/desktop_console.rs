use std::{ffi::c_void, fs::File, io, os::windows::io::AsRawHandle};

#[link(name = "kernel32")]
extern "system" {
    fn SetStdHandle(kind: u32, handle: *mut c_void) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(window: *mut c_void, text: *const u16, caption: *const u16, flags: u32) -> i32;
}

pub fn initialize() -> io::Result<File> {
    let executable = std::env::current_exe()?;
    let path = executable.with_file_name("console.log");

    let file = File::create(path)?;
    for kind in [(-11_i32) as u32, (-12_i32) as u32] {
        if unsafe { SetStdHandle(kind, file.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(file)
}

pub fn report_error(error: &io::Error) {
    let text: Vec<u16> = format!("Cannot open console.log beside the executable: {error}\0")
        .encode_utf16()
        .collect();
    let caption: Vec<u16> = "Meteor\0".encode_utf16().collect();

    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10) };
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "child process used by console_log_replaces_previous_run"]
    fn logging_child() {
        let _file = super::initialize().unwrap();
        println!("stdout marker");
        eprintln!("stderr marker");
        panic!("panic marker");
    }

    #[test]
    fn console_log_replaces_previous_run() {
        let directory = std::env::temp_dir().join(format!(
            "meteor-console-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("logging-test.exe");
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let log = directory.join("console.log");
        for _ in 0..2 {
            std::fs::write(&log, "previous run must disappear".repeat(100)).unwrap();
            let result = std::process::Command::new(&executable)
                .args(["--ignored", "--nocapture", "logging_child"])
                .current_dir(std::env::temp_dir())
                .output()
                .unwrap();
            assert!(!result.status.success(), "child deliberately panics");
            let content = std::fs::read_to_string(&log).unwrap();
            assert!(content.contains("stdout marker"), "{content}");
            assert!(content.contains("stderr marker"), "{content}");
            assert!(content.contains("panic marker"), "{content}");
            assert!(!content.contains("previous run"), "{content}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
