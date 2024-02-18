use std::time::SystemTime;

use btleplug::api::{
    Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    ValueNotification, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use btleplug::Result;
use bytemuck::{Pod, Zeroable};
use futures::stream::BoxStream;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use uuid::{uuid, Uuid};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut watch = get_infinitime().await?.unwrap();

    let version = watch.version().await?;
    println!("version {version}");

    let entries = watch.list_dir("/").await?;
    for e in entries {
        println!("{e:?}");
    }

    let settings_dat = watch.read_file("settings.dat").await?;
    println!("{settings_dat:?}");

    let to_write = std::fs::read("../../../Desktop/1377153351m.ldr").unwrap();

    watch.delete_file("/1377153351m.ldr").await?;
    watch.write_file("/1377153351m.ldr", &to_write, 0).await?;
    let written = watch.read_file("/1377153351m.ldr").await?;

    assert_eq!(to_write.len(), written.len());
    assert_eq!(to_write[..64], written[..64]);
    assert_eq!(
        to_write[to_write.len() - 64..],
        written[written.len() - 64..],
    );
    assert_eq!(to_write, written);

    Ok(())
}

async fn get_infinitime() -> Result<Option<InfiniTime>> {
    let manager = Manager::new().await?;
    let adapter = manager.adapters().await?[0].clone();

    adapter.start_scan(ScanFilter::default()).await?;

    let mut events = adapter.events().await?;
    while let Some(event) = events.next().await {
        let (CentralEvent::DeviceDiscovered(id)
        | CentralEvent::DeviceUpdated(id)
        | CentralEvent::DeviceConnected(id)
        | CentralEvent::DeviceDisconnected(id)
        | CentralEvent::ManufacturerDataAdvertisement { id, .. }
        | CentralEvent::ServiceDataAdvertisement { id, .. }
        | CentralEvent::ServicesAdvertisement { id, .. }) = event;

        let Ok(peripheral) = adapter.peripheral(&id).await else {
            continue;
        };
        let Some(properties) = peripheral.properties().await? else {
            continue;
        };
        if properties.local_name.as_deref() != Some("InfiniTime") {
            continue;
        }

        adapter.stop_scan().await?;

        let watch = InfiniTime::new(peripheral).await?;
        return Ok(Some(watch));
    }

    Ok(None)
}

pub const VERSION: Uuid = uuid!("adaf0100-4669-6c65-5472-616e73666572");
pub const TRANSFER: Uuid = uuid!("adaf0200-4669-6c65-5472-616e73666572");

pub struct InfiniTime {
    peripheral: Peripheral,
    notifications: BoxStream<'static, ValueNotification>,
    version_c: Characteristic,
    transfer_c: Characteristic,
}

const MAX_PAYLOAD: u32 = 0xE7;

impl InfiniTime {
    pub async fn new(peripheral: Peripheral) -> Result<Self> {
        peripheral.connect().await?;
        peripheral.discover_services().await?;

        let notifications = peripheral.notifications().await?;

        let characteristics = peripheral.characteristics();
        let version_c = characteristics.iter().find(|c| c.uuid == VERSION).unwrap();
        let transfer_c = characteristics.iter().find(|c| c.uuid == TRANSFER).unwrap();

        peripheral.subscribe(&transfer_c).await?;

        Ok(Self {
            peripheral,
            notifications,
            version_c: version_c.clone(),
            transfer_c: transfer_c.clone(),
        })
    }

    pub async fn version(&self) -> Result<u32> {
        let mut bytes = self.peripheral.read(&self.version_c).await?;
        bytes.resize(std::mem::size_of::<u32>(), 0_u8);
        let four_bytes = bytes.try_into().unwrap();
        Ok(u32::from_le_bytes(four_bytes))
    }

    async fn send(&self, f: impl FnOnce(&mut Vec<u8>)) -> Result<()> {
        let mut buf = Vec::new();
        f(&mut buf);
        self.peripheral
            .write(&self.transfer_c, &buf, WriteType::WithoutResponse)
            .await?;
        Ok(())
    }

    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        assert!(path.len() <= u16::MAX as usize);

        self.send(|buf| {
            buf.push(0x50);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        let mut entries = vec![];
        while let Some(notif) = self.notifications.next().await {
            let n = std::mem::size_of::<Response<RawDirEntry>>();
            let (header, path) = notif.value.split_at(n);

            let raw: &Response<RawDirEntry> = bytemuck::from_bytes(header);
            assert_eq!(raw.command, 0x51);
            assert_eq!(raw.status, 1);
            assert_eq!(raw.body.entry_number as usize, entries.len());
            assert_eq!(raw.body.path_len as usize, path.len());

            let path = String::from_utf8_lossy(path).into_owned();

            entries.push(DirEntry {
                flags: raw.body.flags,
                timestamp: raw.body.timestamp,
                size: raw.body.size,
                path,
            });

            if raw.body.entry_number == raw.body.entry_count {
                break;
            }
        }

        Ok(entries)
    }

    pub async fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        assert!(path.len() <= u16::MAX as usize);

        let mut offset = 0_u32;

        self.send(|buf| {
            buf.push(0x10);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(offset.to_le_bytes());
            buf.extend(MAX_PAYLOAD.to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        let mut contents = Vec::new();
        while let Some(notif) = self.notifications.next().await {
            let n = std::mem::size_of::<Response<FileChunk>>();
            let (header, payload) = notif.value.split_at(n);

            let response: &Response<FileChunk> = bytemuck::from_bytes(header);

            assert_eq!(response.command, 0x11);
            assert_eq!(response.status, 1);
            assert_eq!({ response.body.offset }, offset);
            assert_eq!(response.body.current_len as usize, payload.len());

            contents.extend(payload);
            println!("{} bytes, total {}", payload.len(), contents.len());
            if contents.len() == response.body.total_len as usize {
                break;
            }

            offset += response.body.current_len;
            self.send(|buf| {
                buf.push(0x12);
                buf.push(0x01);
                buf.extend([0, 0]);
                buf.extend(offset.to_le_bytes());
                buf.extend(MAX_PAYLOAD.to_le_bytes());
            })
            .await?;
        }

        Ok(contents)
    }

    pub async fn write_file(
        &mut self,
        path: &str,
        data: &[u8],
        timestamp: impl Timestamp,
    ) -> Result<()> {
        assert!(path.len() <= u16::MAX as usize);
        assert!(data.len() <= u32::MAX as usize);

        let mut offset = 0_u32;

        self.send(|buf| {
            buf.push(0x20);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(offset.to_le_bytes());
            buf.extend(timestamp.to_u64().to_le_bytes());
            buf.extend((data.len() as u32).to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        while let Some(notif) = self.notifications.next().await {
            let response: &Response<Receipt> = bytemuck::from_bytes(&notif.value);
            println!("{response:?}");

            assert_eq!(response.command, 0x21);
            assert_eq!(response.status, 1, "bad status");
            // assert_eq!({ response.body.offset }, offset);

            let mut remaining_data = &data[offset as usize..];

            if remaining_data.len() > MAX_PAYLOAD as usize {
                remaining_data = &remaining_data[..MAX_PAYLOAD as usize];
            }

            if remaining_data.is_empty() {
                break;
            }

            println!("sending {} bytes, offset {offset}", remaining_data.len());

            self.send(|buf| {
                buf.push(0x22);
                buf.push(1);
                buf.extend([0, 0]);
                buf.extend(offset.to_le_bytes());
                buf.extend((remaining_data.len() as u32).to_le_bytes());
                buf.extend(remaining_data);
            })
            .await?;

            offset += remaining_data.len() as u32;
        }

        Ok(())
    }

    pub async fn delete_file(&mut self, path: &str) -> Result<()> {
        assert!(path.len() <= u16::MAX as usize);

        self.send(|buf| {
            buf.push(0x30);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        let notif = self.notifications.next().await.unwrap();
        let response: &Response<()> = bytemuck::from_bytes(&notif.value);
        assert_eq!(response.command, 0x31);
        assert_eq!(response.status, 1);

        Ok(())
    }
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
struct Response<T> {
    command: u8,
    status: i8,
    body: T,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
struct RawDirEntry {
    path_len: u16,
    entry_number: u32,
    entry_count: u32,
    flags: u32,
    timestamp: u64,
    size: u32,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
struct FileChunk {
    _padding: [u8; 2],
    offset: u32,
    total_len: u32,
    current_len: u32,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
struct Receipt {
    _padding: [u8; 2],
    offset: u32,
    timestamp: u64,
    remaining: u32,
}

#[derive(Debug)]
pub struct DirEntry {
    pub flags: u32,
    pub timestamp: u64,
    pub size: u32,
    pub path: String,
}

pub trait Timestamp {
    fn to_u64(&self) -> u64;
}

impl Timestamp for u64 {
    fn to_u64(&self) -> u64 {
        *self
    }
}

impl Timestamp for SystemTime {
    fn to_u64(&self) -> u64 {
        self.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .try_into()
            .unwrap()
    }
}
