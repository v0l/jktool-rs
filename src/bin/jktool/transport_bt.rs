use jk_bms::{Transport, Result, JkError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::runtime::Runtime;

use btleplug::api::{BDAddr, Central, Manager as _, Peripheral as _, ScanFilter, WriteType, Characteristic};
use btleplug::platform::{Manager, Peripheral};
use futures_util::StreamExt;
use uuid::Uuid;

pub struct BtDevice {
    pub name: Option<String>,
    pub address: String,
    pub rssi: Option<i16>,
}

pub fn scan() -> Result<Vec<BtDevice>> {
    let runtime = Runtime::new()
        .map_err(|e| JkError::TransportError(format!("tokio runtime: {}", e)))?;

    runtime.block_on(async {
        let manager = Manager::new().await
            .map_err(|e| JkError::TransportError(format!("bt manager: {}", e)))?;

        let adapters = manager.adapters().await
            .map_err(|e| JkError::TransportError(format!("bt adapters: {}", e)))?;
        let adapter = adapters.into_iter().next()
            .ok_or_else(|| JkError::TransportError("no bluetooth adapter found".to_string()))?;

        adapter.start_scan(ScanFilter::default()).await
            .map_err(|e| JkError::TransportError(format!("bt scan: {}", e)))?;
        tokio::time::sleep(Duration::from_secs(3)).await;

        let peripherals = adapter.peripherals().await
            .map_err(|e| JkError::TransportError(format!("bt peripherals: {}", e)))?;

        let mut devices = Vec::new();
        for peripheral in peripherals {
            let mut name = None;
            let mut rssi = None;

            if let Ok(Some(props)) = peripheral.properties().await {
                name = props.local_name;
                rssi = props.rssi;
            }

            devices.push(BtDevice {
                name,
                address: peripheral.address().to_string(),
                rssi,
            });
        }

        Ok(devices)
    })
}

pub struct BluetoothTransport {
    target: String,
    char_uuid_str: String,
    runtime: Option<Runtime>,
    peripheral: Option<Peripheral>,
    characteristic: Option<Characteristic>,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl BluetoothTransport {
    pub fn new(target: &str, topts: Option<&str>) -> Self {
        let char_uuid = topts.map(|s| {
            let trimmed = s.trim();
            if trimmed.starts_with("0x") {
                let hex = &trimmed[2..];
                if hex.len() <= 4 {
                    format!("0000{}-0000-1000-8000-00805f9b34fb", hex)
                } else {
                    trimmed.to_string()
                }
            } else if trimmed.len() == 36 && trimmed.contains('-') {
                trimmed.to_string()
            } else if trimmed.len() == 4 {
                format!("0000{}-0000-1000-8000-00805f9b34fb", trimmed)
            } else {
                format!("0000{}-0000-1000-8000-00805f9b34fb", trimmed)
            }
        }).unwrap_or_else(|| "0000ffe1-0000-1000-8000-00805f9b34fb".to_string());

        Self {
            target: target.to_string(),
            char_uuid_str: char_uuid,
            runtime: None,
            peripheral: None,
            characteristic: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn from_target(target: &str) -> Self {
        let mut parts = target.split(',');
        let mac = parts.next().unwrap_or(target);
        let topts = parts.next();
        Self::new(mac, topts)
    }
}

impl Transport for BluetoothTransport {
    fn open(&mut self) -> Result<()> {
        let runtime = Runtime::new()
            .map_err(|e| JkError::TransportError(format!("tokio runtime: {}", e)))?;

        let target = self.target.clone();
        let char_uuid_str = self.char_uuid_str.clone();
        let buffer = self.buffer.clone();

        let (peripheral, characteristic) = runtime.block_on(async {
            let manager = Manager::new().await
                .map_err(|e| JkError::TransportError(format!("bt manager: {}", e)))?;

            let adapters = manager.adapters().await
                .map_err(|e| JkError::TransportError(format!("bt adapters: {}", e)))?;
            let adapter = adapters.into_iter().next()
                .ok_or_else(|| JkError::TransportError("no bluetooth adapter found".to_string()))?;

            adapter.start_scan(ScanFilter::default()).await
                .map_err(|e| JkError::TransportError(format!("bt scan: {}", e)))?;
            tokio::time::sleep(Duration::from_secs(3)).await;

            let peripherals = adapter.peripherals().await
                .map_err(|e| JkError::TransportError(format!("bt peripherals: {}", e)))?;

            let target_addr: BDAddr = target.parse()
                .map_err(|_| JkError::TransportError(format!("invalid mac address: {}", target)))?;

            let peripheral = peripherals.into_iter()
                .find(|p| p.address() == target_addr)
                .ok_or_else(|| JkError::TransportError(format!("device {} not found", target)))?;

            peripheral.connect().await
                .map_err(|e| JkError::TransportError(format!("bt connect: {}", e)))?;

            peripheral.discover_services().await
                .map_err(|e| JkError::TransportError(format!("bt discover: {}", e)))?;

            let characteristics = peripheral.characteristics();
            let char_uuid = Uuid::parse_str(&char_uuid_str)
                .map_err(|e| JkError::TransportError(format!("invalid uuid: {}", e)))?;

            let characteristic = characteristics.into_iter()
                .find(|c| c.uuid == char_uuid)
                .ok_or_else(|| JkError::TransportError(format!("characteristic {} not found", char_uuid_str)))?;

            peripheral.subscribe(&characteristic).await
                .map_err(|e| JkError::TransportError(format!("bt subscribe: {}", e)))?;

            let mut notifications = peripheral.notifications().await
                .map_err(|e| JkError::TransportError(format!("bt notifications: {}", e)))?;

            let buffer_clone = buffer.clone();
            tokio::spawn(async move {
                while let Some(notification) = notifications.next().await {
                    if let Ok(mut buf) = buffer_clone.lock() {
                        buf.extend_from_slice(&notification.value);
                    }
                }
            });

            Ok::<_, JkError>((peripheral, characteristic))
        })?;

        self.runtime = Some(runtime);
        self.peripheral = Some(peripheral);
        self.characteristic = Some(characteristic);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some(runtime) = self.runtime.take() {
            if let Some(peripheral) = self.peripheral.take() {
                let _ = runtime.block_on(async {
                    let _ = peripheral.disconnect().await;
                });
            }
        }
        self.characteristic = None;
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        let runtime = self.runtime.as_ref().ok_or(JkError::TransportNotInitialized)?;
        let peripheral = self.peripheral.as_ref().ok_or(JkError::TransportNotInitialized)?;
        let characteristic = self.characteristic.as_ref().ok_or(JkError::TransportNotInitialized)?;

        // Clear buffer before write (matches C behavior)
        {
            let mut buf = self.buffer.lock()
                .map_err(|_| JkError::TransportError("mutex poisoned".to_string()))?;
            buf.clear();
        }

        runtime.block_on(async {
            peripheral.write(characteristic, data, WriteType::WithoutResponse)
                .await
                .map_err(|_e| JkError::WriteFailed(0))
        })?;

        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            let mut buffer = self.buffer.lock()
                .map_err(|_| JkError::TransportError("mutex poisoned".to_string()))?;

            if !buffer.is_empty() {
                let len = std::cmp::min(buffer.len(), buf.len());
                buf[..len].copy_from_slice(&buffer[..len]);
                buffer.drain(..len);
                return Ok(len);
            }
            drop(buffer);
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(0)
    }
}
