//! Platform HID transport for the HeartMath emWave2 (VID 0x0E30, PID 0x0008).
//!
//! Desktop uses hidapi. Android uses UsbManager permission + UsbDeviceConnection's
//! fd, then nusb for HID control/interrupt transfers.

pub const REPORT_LEN: usize = 63;

#[cfg(not(target_os = "android"))]
mod platform {
    use hidapi::{HidApi, HidDevice};

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
            let d = Self { _api: api, dev };
            d.session_flag(true)?;
            d.send_command(b"J-\r")?;
            d.send_command(b"SR\r")?;
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

        fn session_flag(&self, on: bool) -> Result<(), String> {
            let cur = self.get_feature(0x72, 2)?;
            let b = cur.get(1).copied().unwrap_or(0);
            self.set_feature(0x72, &[(b & 0x80) | u8::from(on)])
        }

        fn send_command(&self, cmd: &[u8]) -> Result<(), String> {
            let mut buf = [0u8; super::REPORT_LEN];
            buf[0] = 0x53;
            buf[3] = cmd.len() as u8;
            buf[4..4 + cmd.len()].copy_from_slice(cmd);
            let n = self.dev.write(&buf).map_err(|e| e.to_string())?;
            if n < super::REPORT_LEN {
                return Err("short write".to_string());
            }
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
}

#[cfg(target_os = "android")]
mod platform {
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
        let devices = env
            .call_method(&manager, "getDeviceList", "()Ljava/util/HashMap;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        let values = env
            .call_method(&devices, "values", "()Ljava/util/Collection;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;
        let iter = env
            .call_method(&values, "iterator", "()Ljava/util/Iterator;", &[])
            .map_err(jni_error)?
            .l()
            .map_err(jni_error)?;

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
            d.session_flag(true)?;
            d.send_command(b"J-\r")?;
            d.send_command(b"SR\r")?;
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

        fn session_flag(&self, on: bool) -> Result<(), String> {
            let cur = self.get_feature(0x72, 2)?;
            let b = cur.get(1).copied().unwrap_or(0);
            self.set_feature(0x72, &[(b & 0x80) | u8::from(on)])
        }

        fn send_command(&mut self, cmd: &[u8]) -> Result<(), String> {
            let mut buf = [0u8; super::REPORT_LEN];
            buf[0] = 0x53;
            buf[3] = cmd.len() as u8;
            buf[4..4 + cmd.len()].copy_from_slice(cmd);
            self.writer.write_all(&buf).map_err(|e| e.to_string())?;
            self.writer.flush().map_err(|e| e.to_string())
        }

        pub fn read_report(&mut self, _timeout_ms: i32) -> Result<Option<Vec<u8>>, String> {
            let mut buf = [0u8; 64];
            let n = self.reader.read(&mut buf).map_err(|e| e.to_string())?;
            Ok((n > 0).then(|| buf[..n].to_vec()))
        }
    }
}

pub use platform::Device;

#[cfg(target_os = "android")]
pub use platform::AndroidContext;

#[cfg(not(target_os = "android"))]
#[derive(Clone, Copy)]
pub struct AndroidContext;
