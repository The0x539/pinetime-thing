use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use btleplug::Result;
use futures::StreamExt;

use infinitime_resources::InfiniTime;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut watch = get_infinitime().await?.unwrap();

    let version = watch.version().await?;
    println!("version {version}");

    let entries = watch.list_dir("/").await?;
    for e in entries {
        println!("{e:?}");
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
