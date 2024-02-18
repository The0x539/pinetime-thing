use std::time::SystemTime;

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use btleplug::Result;
use futures::StreamExt;

use infinitime_resources::InfiniTime;
use serde::Deserialize;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut watch = get_infinitime().await?.unwrap();

    std::env::set_current_dir("../../../Desktop/resources")?;

    let resources: Resources = serde_json::from_slice(&std::fs::read("./resources.json")?)?;

    watch.create_dir("/fonts", SystemTime::now()).await?;
    watch.create_dir("/images", SystemTime::now()).await?;

    for resource in resources.resources {
        let data = std::fs::read(&resource.filename)?;
        println!("Transferring {} to {}", resource.filename, resource.path);
        let now = SystemTime::now();
        watch.write_file(&resource.path, &data, now).await?;
        println!("Done!");
    }

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub resources: Vec<Resource>,
    pub obsolete_files: Vec<ObsoleteFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub filename: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObsoleteFile {
    pub path: String,
    pub since: String,
}
