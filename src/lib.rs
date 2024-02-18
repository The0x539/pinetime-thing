use std::time::SystemTime;

use btleplug::api::{Characteristic, Peripheral as _, ValueNotification, WriteType};
use btleplug::platform::Peripheral;
use btleplug::Result;
use futures::stream::BoxStream;
use futures::StreamExt;
use uuid::{uuid, Uuid};

mod responses;
use responses::*;

pub struct InfiniTime {
    peripheral: Peripheral,
    notifications: BoxStream<'static, ValueNotification>,
    version_c: Characteristic,
    transfer_c: Characteristic,
}

const MAX_PAYLOAD: u32 = 0xE7;
const VERSION: Uuid = uuid!("adaf0100-4669-6c65-5472-616e73666572");
const TRANSFER: Uuid = uuid!("adaf0200-4669-6c65-5472-616e73666572");

impl InfiniTime {
    pub async fn new(peripheral: Peripheral) -> Result<Self> {
        peripheral.connect().await?;
        peripheral.discover_services().await?;

        let notifications = peripheral.notifications().await?;

        let characteristics = peripheral.characteristics();
        let version_c = characteristics
            .iter()
            .find(|c| c.uuid == VERSION)
            .expect("Could not find version characteristic")
            .clone();

        let transfer_c = characteristics
            .iter()
            .find(|c| c.uuid == TRANSFER)
            .expect("Could not find transfer characteristic")
            .clone();

        peripheral.subscribe(&transfer_c).await?;

        Ok(Self {
            peripheral,
            notifications,
            version_c,
            transfer_c,
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

    async fn recv<T: responses::Body>(&mut self) -> Option<responses::Response<T>> {
        let notif = self.notifications.next().await?;
        let response: &responses::Response<T> = bytemuck::from_bytes(&notif.value);

        assert_eq!(response.command, T::COMMAND);
        assert_eq!(response.status, 1, "bad status");

        Some(*response)
    }

    async fn payload_recv<T: responses::Body>(
        &mut self,
    ) -> Option<(responses::Response<T>, Vec<u8>)> {
        let notif = self.notifications.next().await?;
        let mut data = notif.value;
        let payload = data.split_off(std::mem::size_of::<T>());

        let response: &responses::Response<T> = bytemuck::from_bytes(&data);

        assert_eq!(response.command, T::COMMAND);
        assert_eq!(response.status, 1, "bad status");

        Some((*response, payload))
    }

    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        self.send(|buf| {
            buf.push(0x50);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        let mut entries = vec![];

        while let Some((raw, path)) = self.payload_recv::<RawDirEntry>().await {
            assert_eq!(raw.body.entry_number as usize, entries.len());
            assert_eq!(raw.body.path_len as usize, path.len());

            let path = String::from_utf8_lossy(&path).into_owned();

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

        while let Some((response, payload)) = self.payload_recv::<FileChunk>().await {
            assert_eq!({ response.body.offset }, offset);
            assert_eq!(response.body.current_len as usize, payload.len());

            contents.extend(payload);
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

        while let Some(_response) = self.recv::<WriteReceipt>().await {
            // assert_eq!({ response.body.offset }, offset);

            let mut remaining_data = &data[offset as usize..];

            if remaining_data.len() > MAX_PAYLOAD as usize {
                remaining_data = &remaining_data[..MAX_PAYLOAD as usize];
            }

            if remaining_data.is_empty() {
                break;
            }

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
        self.send(|buf| {
            buf.push(0x30);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        self.recv::<RmReceipt>().await.unwrap();

        Ok(())
    }

    pub async fn create_dir(&mut self, path: &str, timestamp: impl Timestamp) -> Result<()> {
        self.send(|buf| {
            buf.push(0x40);
            buf.push(0);
            buf.extend((path.len() as u16).to_le_bytes());
            buf.extend([0; 4]);
            buf.extend(timestamp.to_u64().to_le_bytes());
            buf.extend(path.as_bytes());
        })
        .await?;

        self.recv::<MkdirReceipt>().await.unwrap();

        Ok(())
    }

    pub async fn move_file(&mut self, from: &str, to: &str) -> Result<()> {
        self.send(|buf| {
            buf.push(0x60);
            buf.push(0);
            buf.extend((from.len() as u16).to_le_bytes());
            buf.extend((to.len() as u16).to_le_bytes());
            buf.extend(from.as_bytes());
            buf.push(0);
            buf.extend(to.as_bytes());
        })
        .await?;

        self.recv::<MvReceipt>().await.unwrap();

        Ok(())
    }
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
