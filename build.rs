fn main() {
    // Dark std-widgets (LineEdit, ScrollView) to match the Figma dark theme.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");
}
