use std::ffi::CString;

pub fn run() {
    set_process_name("Botty-Guy");
    println!("helloworld");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn set_process_name(name: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::prctl(libc::PR_SET_NAME, c_name.as_ptr() as usize, 0, 0, 0);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(c_name) = CString::new(name) {
            unsafe {
                libc::pthread_setname_np(c_name.as_ptr());
            }
        }
    }
}
