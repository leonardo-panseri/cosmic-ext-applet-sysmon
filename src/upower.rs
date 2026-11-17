// SPDX-License-Identifier: MPL-2.0

//! Whether the machine runs on battery, from `UPower`'s `OnBattery` property (D2). Changes arrive
//! as `PropertiesChanged` signals on the system bus, so watching costs nothing between them.

use cosmic::iced::futures::StreamExt;

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower"
)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

/// Calls `on_change` with the current `OnBattery` value and then with every change. Returns
/// when `UPower` can't be reached or the signal stream ends; the caller then keeps assuming AC.
pub async fn watch_on_battery(mut on_change: impl FnMut(bool)) {
    let Ok(connection) = zbus::Connection::system().await else {
        return;
    };
    let Ok(proxy) = UPowerProxy::new(&connection).await else {
        return;
    };
    // Subscribe before the first read, so no change falls between the two.
    let mut changes = proxy.receive_on_battery_changed().await;
    if let Ok(on_battery) = proxy.on_battery().await {
        on_change(on_battery);
    }
    while let Some(change) = changes.next().await {
        if let Ok(on_battery) = change.get().await {
            on_change(on_battery);
        }
    }
}
