use jk_bms::{Transport, Result, JkError};
use std::time::Duration;
use std::os::unix::io::RawFd;

/// CAN frame structure for Linux CAN sockets
#[repr(C)]
#[derive(Debug, Clone)]
struct CanFrame {
    can_id: u32,
    data: [u8; 8],
    len: u8,
}

const CAN_SFF_MASK: u32 = 0x1FFFFFF;

#[derive(Debug, Clone)]
pub struct CanTransport {
    interface: String,
    rx_id: u32,
    tx_id: u32,
    fd: Option<RawFd>,
}

impl CanTransport {
    pub fn new(interface: &str, rx_id: u32, tx_id: u32) -> Self {
        Self {
            interface: interface.to_string(),
            rx_id,
            tx_id,
            fd: None,
        }
    }

    pub fn from_target(target: &str) -> Result<Self> {
        // Parse format: can:/dev/can0,0x18ff0000,0x18fe0000
        // or: can:can0,0x18ff0000,0x18fe0000
        let parts: Vec<&str> = target.split(',').collect();
        if parts.len() < 3 {
            return Err(JkError::TransportError(
                "Invalid CAN target format. Use: can:/dev/can0,rx_id,tx_id".to_string()
            ));
        }

        let interface = parts[0].trim_start_matches("can:");
        let rx_id = u32::from_str_radix(parts[1].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid RX CAN ID".to_string()))?;
        let tx_id = u32::from_str_radix(parts[2].trim_start_matches("0x"), 16)
            .map_err(|_| JkError::TransportError("Invalid TX CAN ID".to_string()))?;

        Ok(Self::new(interface, rx_id, tx_id))
    }

    fn open_socket(&self) -> Result<RawFd> {
        // Get interface index using ioctl
        let if_index = unsafe {
            let mut ifreq: libc::ifreq = std::mem::zeroed();
            let name_bytes = self.interface.as_bytes();
            // Copy bytes - ifreq.ifr_name type varies by architecture
            for (i, &b) in name_bytes.iter().enumerate() {
                if i < libc::IFNAMSIZ {
                    ifreq.ifr_name[i] = b as _;
                }
            }
            
            let test_fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if test_fd < 0 {
                return Err(JkError::TransportError("Failed to create socket for interface lookup".to_string()));
            }
            
            let ret = libc::ioctl(test_fd, libc::SIOCGIFINDEX, &mut ifreq);
            libc::close(test_fd);
            
            if ret < 0 {
                return Err(JkError::TransportError(format!(
                    "Failed to get interface index for {}: errno {}",
                    self.interface, errno()
                )));
            }
            
            // Access ifru_ifindex from the union
            ifreq.ifr_ifru.ifru_ifindex
        };

        if if_index < 0 {
            return Err(JkError::TransportError(format!(
                "Failed to get interface index for {}: invalid index",
                self.interface
            )));
        }

        // Create CAN socket
        let fd = unsafe {
            libc::socket(libc::AF_CAN, libc::SOCK_RAW, libc::CAN_RAW)
        };

        if fd < 0 {
            return Err(JkError::TransportError(format!(
                "Failed to create CAN socket: errno {}", errno()
            )));
        }

        // Create sockaddr_can
        #[repr(C)]
        struct SockaddrCan {
            sa_family: u16,
            can_ifindex: i32,
            _pad: [u8; 8],
        }
        
        let sockaddr = SockaddrCan {
            sa_family: libc::AF_CAN as u16,
            can_ifindex: if_index,
            _pad: [0; 8],
        };

        let ret = unsafe {
            libc::connect(
                fd,
                &sockaddr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<SockaddrCan>() as libc::socklen_t,
            )
        };

        if ret < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to connect CAN socket to {}: errno {}",
                self.interface, errno()
            )));
        }

        // Set RX filter for our message ID
        let filter = libc::can_filter {
            can_id: self.rx_id & CAN_SFF_MASK,
            can_mask: CAN_SFF_MASK,
        };

        let ret = unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_CAN_BASE,
                libc::CAN_RAW_FILTER,
                &filter as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::can_filter>() as libc::socklen_t,
            )
        };

        if ret < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to set CAN filter: errno {}", errno()
            )));
        }

        // Set non-blocking mode
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            unsafe { libc::close(fd); }
            return Err(JkError::TransportError(format!(
                "Failed to set non-blocking mode: errno {}", errno()
            )));
        }

        Ok(fd)
    }

    fn write_can(&mut self, frame: &CanFrame) -> Result<usize> {
        if let Some(fd) = self.fd {
            let bytes_written = unsafe {
                libc::write(
                    fd,
                    &frame as *const _ as *const libc::c_void,
                    std::mem::size_of::<CanFrame>()
                )
            };

            if bytes_written < 0 {
                return Err(JkError::WriteFailed(errno()));
            }

            Ok(bytes_written as usize)
        } else {
            Err(JkError::TransportNotInitialized)
        }
    }

    fn read_can(&mut self, frame: &mut CanFrame) -> Result<usize> {
        if let Some(fd) = self.fd {
            let bytes_read = unsafe {
                libc::read(
                    fd,
                    frame as *mut _ as *mut libc::c_void,
                    std::mem::size_of::<CanFrame>()
                )
            };

            if bytes_read < 0 {
                let err = errno();
                if err == libc::EAGAIN || err == libc::EWOULDBLOCK {
                    return Ok(0); // No data available
                }
                return Err(JkError::ReadFailed(err));
            }

            Ok(bytes_read as usize)
        } else {
            Err(JkError::TransportNotInitialized)
        }
    }
}

fn errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

impl Transport for CanTransport {
    fn open(&mut self) -> Result<()> {
        let fd = self.open_socket()?;
        
        // Bring up the CAN interface (may need root privileges)
        use std::process::Command;
        let _ = Command::new("ip")
            .args(["link", "set", &self.interface, "up"])
            .output();
            
        self.fd = Some(fd);
        log::info!("CAN transport opened on {} with RX=0x{:07X}, TX=0x{:07X}", 
                   self.interface, self.rx_id, self.tx_id);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some(fd) = self.fd {
            unsafe { libc::close(fd); }
            self.fd = None;
        }
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.fd.is_none() {
            return Err(JkError::TransportNotInitialized);
        }

        // JK BMS CAN protocol uses standard 8-byte CAN frames
        // We may need to fragment larger messages
        if data.len() > 8 {
            // For now, just send the first 8 bytes
            // In a full implementation, we'd implement CAN multi-frame protocol
            log::warn!("CAN frame too large ({} bytes), truncating to 8 bytes", data.len());
        }

        let mut frame = CanFrame {
            can_id: self.tx_id & CAN_SFF_MASK,
            data: [0u8; 8],
            len: data.len().min(8) as u8,
        };

        frame.data[..frame.len as usize].copy_from_slice(&data[..frame.len as usize]);

        match self.write_can(&frame) {
            Ok(_) => Ok(frame.len as usize),
            Err(e) => Err(e),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.fd.is_none() {
            return Err(JkError::TransportNotInitialized);
        }

        let mut frame = CanFrame {
            can_id: 0,
            data: [0u8; 8],
            len: 0,
        };

        let start = std::time::Instant::now();
        let mut total = 0;

        while total < buf.len() && start.elapsed() < Duration::from_secs(3) {
            match self.read_can(&mut frame) {
                Ok(0) => {
                    // No data, wait a bit
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Ok(_) => {
                    if frame.len > 0 {
                        let copy_len = frame.len as usize;
                        buf[total..total + copy_len].copy_from_slice(&frame.data[..copy_len]);
                        total += copy_len;
                        
                        // For CAN, we typically get one frame at a time
                        // Break after receiving one complete frame
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        if total > 0 {
            log::debug!("CAN read {} bytes", total);
        }
        
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_transport_parsing() {
        let transport = CanTransport::from_target("can:/dev/can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "/dev/can0");
        assert_eq!(t.rx_id, 0x18ff0000);
        assert_eq!(t.tx_id, 0x18fe0000);
    }

    #[test]
    fn test_can_transport_parsing_without_dev() {
        let transport = CanTransport::from_target("can:can0,0x18ff0000,0x18fe0000");
        assert!(transport.is_ok());
        let t = transport.unwrap();
        assert_eq!(t.interface, "can0");
    }

    #[test]
    fn test_can_transport_invalid_format() {
        let transport = CanTransport::from_target("can:/dev/can0");
        assert!(transport.is_err());
    }

    #[test]
    fn test_can_transport_invalid_ids() {
        let transport = CanTransport::from_target("can:/dev/can0,invalid,0x18fe0000");
        assert!(transport.is_err());
    }
}
