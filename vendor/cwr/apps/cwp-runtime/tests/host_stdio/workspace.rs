use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn managed_workspace(root: &Path) -> PathBuf {
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("create managed workspace");
    for (name, contents) in [
        ("CWR.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './root-coworker.toml'\nas = 'root'\n[[imports]]\npath = './sh-coworker.toml'\nas = 'sh'\n"),
        ("root-coworker.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'root'\nslug = 'root'\n"),
        ("sh-coworker.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Shell'\nslug = 'sh'\n[[roles]]\npath = './sh-role.toml'\n[[integrations]]\npath = './local-sh.toml'\nas = 'local'\n"),
        ("sh-role.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n"),
        ("local-sh.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n"),
    ] { fs::write(workspace.join(name), contents).expect("write managed declaration"); }
    workspace.join("CWR.toml")
}
