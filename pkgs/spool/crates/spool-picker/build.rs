fn main() {
  // Embed everything (the UI font included) so the picker never reads
  // resources from disk at runtime.
  let config = slint_build::CompilerConfiguration::new()
    .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
  slint_build::compile_with_config("ui/picker.slint", config).expect("compile ui/picker.slint");
}
