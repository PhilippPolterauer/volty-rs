//! Battery sources consumed by the tray. Each source implements
//! [`volty_battery::BatteryProvider`]; the tray never sees protocol details.

mod hid;
mod name_cache;
mod xinput;

use crate::state::DeviceState;
use volty_battery::BatteryProvider;

/// The providers compiled into this build, in polling order.
pub fn default_providers(state: Option<DeviceState>) -> Vec<Box<dyn BatteryProvider>> {
    let mut providers: Vec<Box<dyn BatteryProvider>> = vec![Box::new(hid::HidProvider::new(state))];
    providers.push(Box::new(xinput::XInputProvider));
    #[cfg(feature = "bluetooth")]
    providers.push(Box::new(volty_bluetooth::BluetoothProvider));
    providers
}
