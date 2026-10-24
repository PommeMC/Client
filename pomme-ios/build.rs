fn main() {
    // MoltenVK, which pyronyx links as `vulkan`, is a static C++ library
    // drawing through these frameworks.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        println!("cargo:rustc-link-lib=c++");
        for framework in [
            "Metal",
            "QuartzCore",
            "IOSurface",
            "UIKit",
            "Foundation",
            "CoreGraphics",
        ] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
