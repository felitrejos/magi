use crate::proto::CoreKind;

#[cfg(target_os = "macos")]
pub fn kinds() -> Vec<CoreKind> {
    mac::kinds().unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub fn kinds() -> Vec<CoreKind> {
    let read = |name: &str| std::fs::read_to_string(format!("/sys/devices/{name}/cpus")).ok();
    match (read("cpu_core"), read("cpu_atom")) {
        (Some(performance), Some(efficiency)) => hybrid(&performance, &efficiency),
        _ => Vec::new(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn hybrid(performance: &str, efficiency: &str) -> Vec<CoreKind> {
    let (Some(performance), Some(efficiency)) = (cpu_list(performance), cpu_list(efficiency))
    else {
        return Vec::new();
    };
    let count = performance.len() + efficiency.len();
    let kinds: Vec<Option<CoreKind>> = (0..count)
        .map(|cpu| {
            if performance.contains(&cpu) {
                Some(CoreKind::Performance)
            } else if efficiency.contains(&cpu) {
                Some(CoreKind::Efficiency)
            } else {
                None
            }
        })
        .collect();
    kinds.into_iter().collect::<Option<_>>().unwrap_or_default()
}

#[cfg(any(target_os = "linux", test))]
fn cpu_list(text: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|part| !part.is_empty()) {
        match part.split_once('-') {
            Some((first, last)) => cpus.extend(first.parse::<usize>().ok()?..=last.parse().ok()?),
            None => cpus.push(part.parse().ok()?),
        }
    }
    Some(cpus)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CStr, c_char, c_void};

    use core_foundation::base::{CFType, TCFType, kCFAllocatorDefault};
    use core_foundation::data::CFData;
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};

    use crate::proto::CoreKind;

    type IoObject = u32;

    const MAIN_PORT: u32 = 0;
    const CPUS: &CStr = c"IODeviceTree:/cpus";
    const PLANE: &CStr = c"IODeviceTree";

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryEntryFromPath(main_port: u32, path: *const c_char) -> IoObject;
        fn IORegistryEntryGetChildIterator(
            entry: IoObject,
            plane: *const c_char,
            iterator: *mut IoObject,
        ) -> i32;
        fn IOIteratorNext(iterator: IoObject) -> IoObject;
        fn IORegistryEntryCreateCFProperty(
            entry: IoObject,
            key: CFStringRef,
            allocator: *const c_void,
            options: u32,
        ) -> *const c_void;
        fn IOObjectRelease(object: IoObject) -> i32;
    }

    struct Object(IoObject);

    impl Drop for Object {
        fn drop(&mut self) {
            unsafe { IOObjectRelease(self.0) };
        }
    }

    fn property(entry: &Object, key: &str) -> Option<CFType> {
        let key = CFString::new(key);
        let value = unsafe {
            IORegistryEntryCreateCFProperty(
                entry.0,
                key.as_concrete_TypeRef(),
                kCFAllocatorDefault.cast(),
                0,
            )
        };
        (!value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    pub fn kinds() -> Option<Vec<CoreKind>> {
        let cpus = Object(unsafe { IORegistryEntryFromPath(MAIN_PORT, CPUS.as_ptr()) });
        if cpus.0 == 0 {
            return None;
        }
        let mut iterator = 0;
        if unsafe { IORegistryEntryGetChildIterator(cpus.0, PLANE.as_ptr(), &mut iterator) } != 0 {
            return None;
        }
        let iterator = Object(iterator);
        let mut found: Vec<(usize, CoreKind)> = Vec::new();
        loop {
            let cpu = Object(unsafe { IOIteratorNext(iterator.0) });
            if cpu.0 == 0 {
                break;
            }
            let id = property(&cpu, "logical-cpu-id")?
                .downcast::<CFNumber>()?
                .to_i64()?;
            let cluster = property(&cpu, "cluster-type")?.downcast::<CFData>()?;
            let kind = match cluster.bytes().first()? {
                b'P' => CoreKind::Performance,
                b'E' => CoreKind::Efficiency,
                _ => return None,
            };
            found.push((usize::try_from(id).ok()?, kind));
        }
        found.sort_by_key(|(id, _)| *id);
        found
            .iter()
            .enumerate()
            .all(|(index, (id, _))| index == *id)
            .then(|| found.into_iter().map(|(_, kind)| kind).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hybrid_intel_lists_its_cores_by_kind_and_a_gap_means_unknown() {
        use CoreKind::{Efficiency, Performance};
        assert_eq!(
            hybrid("0-1,4\n", "2-3\n"),
            [
                Performance,
                Performance,
                Efficiency,
                Efficiency,
                Performance
            ]
        );
        assert!(hybrid("0-1", "3").is_empty());
        assert!(hybrid("0-x", "2").is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_names_as_many_performance_and_efficiency_cores_as_it_reports() {
        let count = |key: &str| -> usize {
            let output = std::process::Command::new("sysctl")
                .args(["-n", key])
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .unwrap()
        };
        let kinds = kinds();
        let of = |kind: CoreKind| kinds.iter().filter(|found| **found == kind).count();
        assert_eq!(of(CoreKind::Performance), count("hw.perflevel0.logicalcpu"));
        assert_eq!(of(CoreKind::Efficiency), count("hw.perflevel1.logicalcpu"));
    }
}
