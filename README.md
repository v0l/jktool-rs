# jktool-rs

Rust implementation of [jktool](https://github.com/v0l/jktool) — a command-line tool for communicating with JIKONG (JK) Battery Management Systems.

Supports the JK02 (24S/32S) and JK04 protocol variants, with automatic detection based on the BMS model string. Provides a reusable library (`jk_bms`) and a CLI (`jktool`).

## Features

- **Multi-transport**: Serial, Bluetooth (optional), with the same `transport:target` syntax as the original
- **Protocol support**: JK02_24S, JK02_32S (PB2/BD/HY series), JK04 (auto-detected from info frame)
- **Live data**: Cell voltages, temperatures, SOC/SOH, power, errors, MOSFET states
- **Settings read/write**: Full register map with human-readable names, scaling, and write frame generation
- **Output formats**: Text, CSV, JSON (with pretty-print)
- **BLE frame assembly**: Handles fragmented BLE notifications with CRC verification
- **Frame assembler**: Reassembles 300-byte JK frames from small BLE MTU chunks

## Building

```bash
cargo build
```

With Bluetooth support:

```bash
cargo build --features bluetooth
```

## Usage

Transports are specified as `transport:target[,options]` — same syntax as the original jktool.

### Serial

```bash
jktool -t serial:/dev/ttyUSB0,9600
```

### Bluetooth

```bash
jktool -t bt:01:02:03:04:05:06,ffe1
```

### Read live data (default)

```bash
jktool -t serial:/dev/ttyUSB0,9600
```

### Read settings

```bash
jktool -t bt:01:02:03:04:05:06 settings
```

### Write a setting

```bash
jktool -t serial:/dev/ttyUSB0,9600 set max_charge_current 50.0
jktool -t bt:01:02:03:04:05:06 set charging on
```

### List supported settings

```bash
jktool list-settings
```

### Output to file (JSON)

```bash
jktool -t serial:/dev/ttyUSB0,9600 -f json -o pack.json
```

### Scan for Bluetooth devices

```bash
jktool scan
```

## Library

The `jk_bms` crate provides the core protocol implementation:

```rust
use jk_bms::{MybmmPack, FrameAssembler, getdata, get_info_command, get_cell_info_command};

let mut pack = MybmmPack::new("pack1");
let mut assembler = FrameAssembler::new();

// Feed raw bytes from any transport
assembler.feed(&bytes);
if let Some(frame) = assembler.try_decode() {
    let flags = getdata(&mut pack, &frame);
    println!("Voltage: {:.3} V", pack.voltage);
    println!("Cells: {}", pack.cells);
}
```

## Protocol versions

| Version | Models | Cell voltages | Max cells |
|---------|--------|---------------|-----------|
| JK02_24S | JK-B2AxxS | 2-byte LE mV | 24 |
| JK02_32S | JK_PB2, JK-BD, JK_HY | 2-byte LE mV (16-byte offset) | 32 |
| JK04 | Older JK-BMS | IEEE 754 float | 24 |

Auto-detected from the BMS info frame model string.

## License

MIT
