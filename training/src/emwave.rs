//! Platform HID transport for the HeartMath emWave2 (VID 0x0E30, PID 0x0008).
//!
//! Desktop uses hidapi. Android uses UsbManager permission + UsbDeviceConnection's
//! fd, then nusb for HID control/interrupt transfers.


#[cfg(not(target_os = "android"))]
mod platform {
    use hidapi::{HidApi, HidDevice};
    use crate::protocol::{HidTransport, REPORT_LEN};

    const VID: u16 = 0x0e30;
    const PID: u16 = 0x0008;

    pub struct Device {
        _api: HidApi,
        dev: HidDevice,
    }

    impl Device {
        pub fn open_and_start(_platform: super::AndroidContext) -> Result<Self, String> {
            let api = HidApi::new().map_err(|e| e.to_string())?;
            let dev = api
                .open(VID, PID)
                .map_err(|_| "emWave2 not found (plug it in; check HID access)".to_string())?;
            let mut d = Self { _api: api, dev };
            d.start_session()?;
            Ok(d)
        }

        fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String> {
            let mut buf = vec![0u8; size + 1];
            buf[0] = report_id;
            let n = self.dev.get_feature_report(&mut buf).map_err(|e| e.to_string())?;
            Ok(buf[..n].to_vec())
        }

        fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String> {
            let mut buf = vec![report_id];
            buf.extend_from_slice(payload);
            self.dev.send_feature_report(&buf).map_err(|e| e.to_string())?;
            Ok(())
        }


        pub fn read_report(&self, timeout_ms: i32) -> Result<Option<Vec<u8>>, String> {
            let mut buf = [0u8; 64];
            match self.dev.read_timeout(&mut buf, timeout_ms) {
                Ok(n) if n > 0 => Ok(Some(buf[..n].to_vec())),
                Ok(_) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }
    }
    impl HidTransport for Device {
        fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String> {
            Device::get_feature(self, report_id, size)
        }

        fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String> {
            Device::set_feature(self, report_id, payload)
        }

        fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), String> {
            let n = self.dev.write(report).map_err(|e| e.to_string())?;
            if n < REPORT_LEN {
                return Err("short write".to_string());
            }
            Ok(())
        }
    }
}

#[cfg(target_os = "android")]
mod platform {
    use crate::protocol::{HidTransport, REPORT_LEN};
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::time::Duration;

    use jni::objects::{JObject, JValue};
    use jni::sys::jobject;
    use jni::JavaVM;
    use nusb::transfer::{ControlIn, ControlOut, ControlType, In, Interrupt, Out, Recipient};
    use nusb::MaybeFuture;
    const VID: i32 = 0x0e30;
    const PID: i32 = 0x0008;

    /// Raw JVM/activity handles are valid for the AndroidApp lifetime and are
    /// copied into the reader thread. AndroidApp exposes these as global refs.
    #[derive(Clone, Copy)]
    pub struct AndroidContext {
        pub vm: usize,
        pub activity: usize,
    }
    pub fn set_keep_screen_on(ctx: AndroidContext, enabled: bool) -> Result<(), String> {
        let vm = unsafe { JavaVM::from_raw(ctx.vm as *mut _) }.map_err(jni_error)?;
        let mut env = vm.attach_current_thread().map_err(jni_error)?;
        let activity = unsafe { JObject::from_raw(ctx.activity as jobject) };
        if activity.is_null() {
            return Err("Android activity reference is null".into());
        }
        let window = env
            .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if window.is_null() {
            return Err("Android window reference is null".into());
        }
        let flags = 0x0000_0080; // FLAG_KEEP_SCREEN_ON
        let method = if enabled { "addFlags" } else { "clearFlags" };
        env.call_method(&window, method, "(I)V", &[JValue::Int(flags)])
            .map_err(jni_error)?;
        Ok(())
    }
    unsafe impl Send for AndroidContext {}
    unsafe impl Sync for AndroidContext {}

    pub struct Device {
        _connection: jni::objects::GlobalRef,
        usb: nusb::Device,
        reader: nusb::io::EndpointRead<Interrupt>,
        writer: nusb::io::EndpointWrite<Interrupt>,
    }

    fn jni_error(e: impl std::fmt::Display) -> String {
        format!("Android USB: {e}")
    }

    fn open_usb(ctx: AndroidContext) -> Result<(jni::objects::GlobalRef, nusb::Device), String> {
        let vm = unsafe { JavaVM::from_raw(ctx.vm as *mut _) }.map_err(jni_error)?;
        let mut env = vm.attach_current_thread().map_err(jni_error)?;
        let activity = unsafe { JObject::from_raw(ctx.activity as jobject) };
        if activity.is_null() {
            return Err("Android activity reference is null".into());
        }
        let service = env.new_string("usb").map_err(jni_error)?;
        let manager = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service.into())],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if manager.is_null() {
            return Err("Android UsbManager unavailable".into());
        }
        let devices = env
            .call_method(&manager, "getDeviceList", "()Ljava/util/HashMap;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if devices.is_null() {
            return Err("Android UsbManager returned no device map".into());
        }
        let values = env
            .call_method(&devices, "values", "()Ljava/util/Collection;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if values.is_null() {
            return Err("Android USB device collection is null".into());
        }
        let iter = env
            .call_method(&values, "iterator", "()Ljava/util/Iterator;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if iter.is_null() {
            return Err("Android USB device iterator is null".into());
        }
        let mut found: Option<JObject> = None;
        loop {
            let has = env
                .call_method(&iter, "hasNext", "()Z", &[])
                .map_err(jni_error)?
                .z()
                .map_err(jni_error)?;
            if !has {
                break;
            }
            let dev = env
                .call_method(&iter, "next", "()Ljava/lang/Object;", &[])
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?;
            if dev.is_null() {
                continue;
            }
            let vendor = env
                .call_method(&dev, "getVendorId", "()I", &[])
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?;
            let product = env
                .call_method(&dev, "getProductId", "()I", &[])
                .map_err(jni_error)?
                .i()
                .map_err(jni_error)?;
            if vendor == VID && product == PID {
                found = Some(dev);
                break;
            }
        }
        let dev = found.ok_or_else(|| "emWave2 not found on Android USB host".to_string())?;
        let permission = env
            .call_method(
                &manager,
                "hasPermission",
                "(Landroid/hardware/usb/UsbDevice;)Z",
                &[JValue::Object(&dev)],
            )
            .map_err(jni_error)?
            .z()
            .map_err(jni_error)?;
        if !permission {
            let action = env.new_string("com.emwave.resonance.USB_PERMISSION").map_err(jni_error)?;
            let intent = env
                .new_object(
                    "android/content/Intent",
                    "(Ljava/lang/String;)V",
                    &[JValue::Object(&action.into())],
                )
                .map_err(jni_error)?;
            let pending = env
                .call_static_method(
                    "android/app/PendingIntent",
                    "getBroadcast",
                    "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
                    &[
                        JValue::Object(&activity),
                        JValue::Int(0),
                        JValue::Object(&intent),
                        JValue::Int(0x0400_0000), // FLAG_IMMUTABLE
                    ],
                )
                .map_err(jni_error)?
                .l()
                .map_err(jni_error)?;
            env.call_method(
                &manager,
                "requestPermission",
                "(Landroid/hardware/usb/UsbDevice;Landroid/app/PendingIntent;)V",
                &[JValue::Object(&dev), JValue::Object(&pending)],
            )
            .map_err(jni_error)?;
            return Err("USB permission requested — approve Android's dialog, then wait".into());
        }

        let connection = env
            .call_method(
                &manager,
                "openDevice",
                "(Landroid/hardware/usb/UsbDevice;)Landroid/hardware/usb/UsbDeviceConnection;",
                &[JValue::Object(&dev)],
            )
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        if connection.is_null() {
            return Err("UsbManager.openDevice returned null".into());
        }
        let fd = env
            .call_method(&connection, "getFileDescriptor", "()I", &[])
            .map_err(jni_error)?
            .i()
            .map_err(jni_error)?;
        if fd < 0 {
            return Err("Android returned an invalid USB file descriptor".into());
        }
        let connection = env.new_global_ref(connection).map_err(jni_error)?;
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) };
        let usb = nusb::Device::from_fd(owned)
            .wait()
            .map_err(|e| e.to_string())?;
        Ok((connection, usb))
    }

    impl Device {
        pub fn open_and_start(ctx: AndroidContext) -> Result<Self, String> {
            let (connection, usb) = open_usb(ctx)?;
            let interface = usb
                .detach_and_claim_interface(0)
                .wait()
                .map_err(|e| e.to_string())?;
            let reader = interface
                .endpoint::<Interrupt, In>(0x81)
                .map_err(|e| e.to_string())?
                .reader(64);
            let writer = interface
                .endpoint::<Interrupt, Out>(0x01)
                .map_err(|e| e.to_string())?
                .writer(64);
            let mut d = Self {
                _connection: connection,
                usb,
                reader,
                writer,
            };
            d.start_session()?;
            Ok(d)
        }

        fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String> {
            let data = self
                .usb
                .control_in(
                    ControlIn {
                        control_type: ControlType::Class,
                        recipient: Recipient::Interface,
                        request: 0x01,
                        value: (0x03u16 << 8) | report_id as u16,
                        index: 0,
                        length: (size + 1) as u16,
                    },
                    Duration::from_secs(2),
                )
                .wait()
                .map_err(|e| e.to_string())?;
            Ok(data)
        }

        fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String> {
            let mut data = vec![report_id];
            data.extend_from_slice(payload);
            self.usb
                .control_out(
                    ControlOut {
                        control_type: ControlType::Class,
                        recipient: Recipient::Interface,
                        request: 0x09,
                        value: (0x03u16 << 8) | report_id as u16,
                        index: 0,
                        data: &data,
                    },
                    Duration::from_secs(2),
                )
                .wait()
                .map_err(|e| e.to_string())
        }


        pub fn read_report(&mut self, _timeout_ms: i32) -> Result<Option<Vec<u8>>, String> {
            let mut buf = [0u8; 64];
            let n = self.reader.read(&mut buf).map_err(|e| e.to_string())?;
            Ok((n > 0).then(|| buf[..n].to_vec()))
        }
    }
    impl HidTransport for Device {
        fn get_feature(&self, report_id: u8, size: usize) -> Result<Vec<u8>, String> {
            Device::get_feature(self, report_id, size)
        }

        fn set_feature(&self, report_id: u8, payload: &[u8]) -> Result<(), String> {
            Device::set_feature(self, report_id, payload)
        }

        fn write_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<(), String> {
            self.writer.write_all(report).map_err(|e| e.to_string())?;
            self.writer.flush().map_err(|e| e.to_string())
        }
    }
}

pub use platform::Device;

#[cfg(target_os = "android")]
pub use platform::{set_keep_screen_on, AndroidContext};

#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy)]
pub struct AndroidContext;

#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
pub fn set_keep_screen_on(_ctx: AndroidContext, _enabled: bool) -> Result<(), String> {
    Ok(())
}
