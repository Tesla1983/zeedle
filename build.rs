fn main() {
    let cfg = slint_build::CompilerConfiguration::new().with_style("native".into());
    slint_build::compile_with_config("ui/app.slint", cfg).expect("slint build failed");
}
