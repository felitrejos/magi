use std::ffi::c_void;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};

type Client = *const c_void;
type Service = *const c_void;
type Event = *const c_void;

const VENDOR_PAGE: i64 = 0xff00;
const TEMPERATURE_USAGE: i64 = 5;
const TEMPERATURE_EVENT: i64 = 15;
const DIE_SENSOR: &str = "tdie";
const MAX_CELSIUS: f64 = 150.0;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: *const c_void) -> Client;
    fn IOHIDEventSystemClientSetMatching(client: Client, matching: *const c_void) -> i32;
    fn IOHIDEventSystemClientCopyServices(client: Client) -> CFArrayRef;
    fn IOHIDServiceClientCopyProperty(service: Service, key: CFStringRef) -> *const c_void;
    fn IOHIDServiceClientCopyEvent(
        service: Service,
        kind: i64,
        options: i32,
        timestamp: i64,
    ) -> Event;
    fn IOHIDEventGetFloatValue(event: Event, field: i32) -> f64;
    fn CFRelease(object: *const c_void);
}

pub struct Sensors {
    client: Client,
}

unsafe impl Send for Sensors {}

impl Sensors {
    pub fn new() -> Option<Self> {
        let client = unsafe { IOHIDEventSystemClientCreate(kCFAllocatorDefault.cast()) };
        if client.is_null() {
            return None;
        }
        let matching = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("PrimaryUsagePage"),
                CFNumber::from(VENDOR_PAGE).as_CFType(),
            ),
            (
                CFString::new("PrimaryUsage"),
                CFNumber::from(TEMPERATURE_USAGE).as_CFType(),
            ),
        ]);
        unsafe { IOHIDEventSystemClientSetMatching(client, matching.as_concrete_TypeRef().cast()) };
        Some(Self { client })
    }

    pub fn hottest_die(&self) -> Option<f32> {
        let services = unsafe { IOHIDEventSystemClientCopyServices(self.client) };
        if services.is_null() {
            return None;
        }
        let services: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(services) };
        let key = CFString::new("Product");
        let mut hottest: Option<f64> = None;
        for service in services.iter() {
            let service: Service = service.as_CFTypeRef();
            let name =
                unsafe { IOHIDServiceClientCopyProperty(service, key.as_concrete_TypeRef()) };
            if name.is_null() {
                continue;
            }
            let name: CFString = unsafe { CFString::wrap_under_create_rule(name.cast()) };
            if !name.to_string().contains(DIE_SENSOR) {
                continue;
            }
            let event = unsafe { IOHIDServiceClientCopyEvent(service, TEMPERATURE_EVENT, 0, 0) };
            if event.is_null() {
                continue;
            }
            let celsius =
                unsafe { IOHIDEventGetFloatValue(event, (TEMPERATURE_EVENT << 16) as i32) };
            unsafe { CFRelease(event) };
            if (0.0..MAX_CELSIUS).contains(&celsius) {
                hottest = Some(hottest.map_or(celsius, |known| known.max(celsius)));
            }
        }
        hottest.map(|celsius| celsius as f32)
    }
}

impl Drop for Sensors {
    fn drop(&mut self) {
        unsafe { CFRelease(self.client) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_mac_reports_a_plausible_cpu_die_temperature() {
        let sensors = Sensors::new().expect("the HID event system is there");
        let celsius = sensors.hottest_die().expect("a CPU die sensor answers");
        assert!((10.0..110.0).contains(&celsius), "{celsius}");
    }
}
