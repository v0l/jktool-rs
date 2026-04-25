use jk_bms::{Transport, Result, JkError};
use std::time::Duration;
use std::os::unix::io::RawFd;

pub struct SerialTransport {
    port_name: String,
    baud_rate: u32,
    fd: RawFd,
}

impl SerialTransport {
    pub fn new(port_name: &str, baud_rate: u32) -> Self {
        Self {
            port_name: port_name.to_string(),
            baud_rate,
            fd: -1,
        }
    }

    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let port = parts.next().unwrap_or("/dev/ttyUSB0");
        let baud = parts.next().and_then(|s| s.parse().ok()).unwrap_or(9600);
        Self::new(port, baud)
    }
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

impl Transport for SerialTransport {
    fn open(&mut self) -> Result<()> {
        use std::ffi::CString;
        let c_path = CString::new(self.port_name.as_bytes())
            .map_err(|_| JkError::TransportError("invalid path".to_string()))?;

        let fd = unsafe {
            libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY)
        };
        if fd < 0 {
            return Err(JkError::TransportError(format!(
                "open {}: errno {}", self.port_name, errno()
            )));
        }

        unsafe {
            let mut tty: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut tty);
            libc::cfmakeraw(&mut tty);
            tty.c_cc[libc::VMIN] = 0;
            tty.c_cc[libc::VTIME] = 50; // 5 seconds

            let rate = match self.baud_rate {
                9600 => libc::B9600,
                19200 => libc::B19200,
                38400 => libc::B38400,
                57600 => libc::B57600,
                115200 => libc::B115200,
                230400 => libc::B230400,
                _ => libc::B9600,
            };
            libc::cfsetispeed(&mut tty, rate);
            libc::cfsetospeed(&mut tty, rate);
            libc::tcsetattr(fd, libc::TCSANOW, &tty);
            libc::tcflush(fd, libc::TCIOFLUSH);
        }

        self.fd = fd;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if self.fd >= 0 {
            unsafe { libc::close(self.fd); }
            self.fd = -1;
        }
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.fd < 0 {
            return Err(JkError::TransportNotInitialized);
        }
        let n = unsafe {
            libc::write(self.fd, data.as_ptr() as *const libc::c_void, data.len())
        };
        if n < 0 {
            return Err(JkError::WriteFailed(n as i32));
        }
        Ok(n as usize)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.fd < 0 {
            return Err(JkError::TransportNotInitialized);
        }

        let mut total = 0usize;
        let start = std::time::Instant::now();
        while total < buf.len() && start.elapsed() < Duration::from_secs(3) {
            let n = unsafe {
                libc::read(self.fd, buf[total..].as_mut_ptr() as *mut libc::c_void, buf.len() - total)
            };
            if n > 0 {
                total += n as usize;
            } else if n < 0 {
                let err = errno();
                if err == libc::EAGAIN || err == libc::EWOULDBLOCK {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                return Err(JkError::ReadFailed(n as i32));
            } else {
                break;
            }
        }
        Ok(total)
    }
}
