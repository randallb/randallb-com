#[cfg(target_os = "macos")]
use std::env;

#[cfg(target_os = "macos")]
pub fn configure_packaged_module_path() -> Result<(), String> {
    let executable = env::current_exe().map_err(|error| error.to_string())?;
    let Some(directory) = executable.parent() else {
        return Err("CWR executable has no containing directory".to_owned());
    };
    let libraries = directory.join("cwr-libs");
    if !libraries.is_dir() {
        return Ok(());
    }
    let modules = libraries.join("i18n");
    if !modules.join("libiconv_std.dylib").is_file()
        || !modules.join("libmapper_std.dylib").is_file()
    {
        return Err("packaged CWR conversion modules are unavailable".to_owned());
    }
    env::set_var("PATH_I18NMODULE", modules);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn configure_packaged_module_path() -> Result<(), String> {
    Ok(())
}
