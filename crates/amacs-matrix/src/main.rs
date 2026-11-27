//! The program through which amacs talks to Matrix.

fn main() {
    // Name the SDK so that it is linked: this is only a trial of the build.
    let _ = std::any::type_name::<matrix_sdk::Client>();
    let _ = std::any::type_name::<matrix_sdk_ui::timeline::Timeline>();
    println!("amacs-matrix {}", env!("CARGO_PKG_VERSION"));
}
