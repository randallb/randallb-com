use std::{env, fs, path::PathBuf};

fn main() {
    let catalog_path = "../configuration/catalog.json";
    println!("cargo:rerun-if-changed={catalog_path}");
    let catalog: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(catalog_path).expect("read configuration catalog"),
    )
    .expect("parse configuration catalog");
    let setting = &catalog["definitions"]["desktop.human_reply_inbox"];
    assert_eq!(setting["kind"], "setting");
    assert_eq!(setting["value_type"], "boolean");
    assert_eq!(setting["client_safe"], true);
    let source = format!(
        "const HUMAN_REPLY_INBOX_DEFAULT: bool = {};\n\
         const HUMAN_REPLY_INBOX_PARAMETER: &str = {:?};\n\
         const HUMAN_REPLY_INBOX_TITLE: &str = {:?};\n\
         const HUMAN_REPLY_INBOX_DESCRIPTION: &str = {:?};\n",
        setting["default"].as_bool().expect("boolean default"),
        setting["source"]["workspace_parameter"]
            .as_str()
            .expect("workspace parameter"),
        setting["title"].as_str().expect("setting title"),
        setting["description"]
            .as_str()
            .expect("setting description"),
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("configuration.rs"),
        source,
    )
    .expect("write generated configuration");
}
