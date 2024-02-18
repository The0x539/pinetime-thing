
use bytemuck::{Pod, Zeroable};

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct Response<T> {
    pub command: u8,
    pub status: i8,
    pub body: T,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct RawDirEntry {
    pub path_len: u16,
    pub entry_number: u32,
    pub entry_count: u32,
    pub flags: u32,
    pub timestamp: u64,
    pub size: u32,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct FileChunk {
    _padding: [u8; 2],
    pub offset: u32,
    pub total_len: u32,
    pub current_len: u32,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct WriteReceipt {
    _padding: [u8; 2],
    pub offset: u32,
    pub timestamp: u64,
    pub remaining: u32,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C, packed)]
pub struct MkdirReceipt {
    _padding: [u8; 6],
    pub timestamp: u64,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct MvReceipt;

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct RmReceipt;

pub trait Body: Pod {
    const COMMAND: u8;
}

impl Body for FileChunk {
    const COMMAND: u8 = 0x11;
}

impl Body for WriteReceipt {
    const COMMAND: u8 = 0x21;
}

impl Body for RmReceipt {
    const COMMAND: u8 = 0x31;
}

impl Body for MkdirReceipt {
    const COMMAND: u8 = 0x41;
}

impl Body for RawDirEntry {
    const COMMAND: u8 = 0x51;
}

impl Body for MvReceipt {
    const COMMAND: u8 = 0x61;
}
