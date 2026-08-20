#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result {
    emwave_trainer::desktop_main()
}

#[cfg(target_os = "android")]
fn main() {}
