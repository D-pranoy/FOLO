//! The application id and the scope the node must create (ADR-0037 D16).
#![cfg(target_os = "linux")]

use folo_capture_linux::appid::{APP_ID, app_id_from_cgroup, transient_scope_name};

#[test]
fn the_scope_name_escapes_the_dashes_as_systemd_does() {
    assert_eq!(
        transient_scope_name("1a2b3c"),
        r"app-io.github.d\x2dpranoy.folo\x2dcapture-1a2b3c.scope"
    );
}

#[test]
fn the_portal_reads_the_id_back_from_the_scope_the_node_creates() {
    let name = transient_scope_name("77f");
    let cgroup = format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{name}\n");
    assert_eq!(app_id_from_cgroup(&cgroup).as_deref(), Some(APP_ID));
}

#[test]
fn services_launchers_and_other_units() {
    let id = r"io.github.d\x2dpranoy.folo\x2dcapture";
    for path in [
        format!("app-{id}.service"),
        format!("app-{id}@3.service"),
        format!("app-gnome-{id}-4242.scope"),
        format!("app-{id}-4242.scope"),
    ] {
        let cgroup = format!("0::/user.slice/app.slice/{path}\n");
        assert_eq!(
            app_id_from_cgroup(&cgroup).as_deref(),
            Some(APP_ID),
            "{path}"
        );
    }
    for cgroup in [
        "0::/user.slice/user-1000.slice/session-3.scope\n",
        "0::/system.slice/folo.service\n",
        "1:name=systemd:/user.slice\n",
        "",
        "0::/app.slice/app-.scope\n",
    ] {
        assert_eq!(app_id_from_cgroup(cgroup), None, "{cgroup:?}");
    }
}

#[test]
fn a_dash_in_the_random_part_cannot_split_the_name() {
    let name = transient_scope_name("ab-cd");
    assert_eq!(
        name,
        r"app-io.github.d\x2dpranoy.folo\x2dcapture-abcd.scope"
    );
    let cgroup = format!("0::/app.slice/{name}\n");
    assert_eq!(app_id_from_cgroup(&cgroup).as_deref(), Some(APP_ID));
}
