//! `config` and configuration handling through the built binary (ports the earlier bash prototype's
//! `test_config_rejects_unknown_keys`).

mod common;

use common::World;

#[test]
fn config_path_follows_the_dir_rules() {
    let mut w = World::new();
    let r = w.run(&["config", "path"]);
    assert_eq!(
        (r.code, r.stdout.as_str()),
        (
            0,
            format!("{}\n", w.config.join("config.toml").display()).as_str()
        )
    );

    // herdr's own variable is used when ours is absent.
    w.unset_env("GLIFFHERDR_CONFIG_DIR");
    w.set_env("HERDR_PLUGIN_CONFIG_DIR", "/herdr/gave/this");
    assert_eq!(
        w.run(&["config", "path"]).stdout,
        "/herdr/gave/this/config.toml\n"
    );

    // With neither, herdr's default location for the plugin under XDG_CONFIG_HOME.
    w.unset_env("HERDR_PLUGIN_CONFIG_DIR");
    assert_eq!(
        w.run(&["config", "path"]).stdout.trim(),
        w.home
            .join(".config/herdr/plugins/config/jaden-hoenes.gliffherdr/config.toml")
            .to_str()
            .unwrap()
    );
}

#[test]
fn config_init_writes_the_template_once() {
    let w = World::new();
    let file = w.config.join("config.toml");
    let first = w.run(&["config", "init"]);
    assert_eq!(
        (first.code, first.stdout.trim()),
        (0, file.to_str().unwrap())
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text, gliffherdr::config::TEMPLATE);
    assert!(text.contains("[connect]") && text.contains("#mode = \"mirror\""));

    std::fs::write(&file, "# mine\n").unwrap();
    let again = w.run(&["config", "init"]);
    assert_eq!(again.code, 0);
    assert_eq!(
        again.stdout,
        format!("already exists: {}\n", file.display())
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# mine\n");
}

#[test]
fn config_check_ok_and_errors() {
    let w = World::new();
    assert_eq!(
        w.run(&["config", "check"]).stdout,
        "ok\n",
        "no file means defaults"
    );
    w.run(&["config", "init"]);
    assert_eq!(
        w.run(&["config", "check"]).stdout,
        "ok\n",
        "the template is valid"
    );

    w.write_config("[connect]\nmode = \"mirror\"\ncolour = \"blue\"\n");
    let r = w.run(&["config", "check"]);
    assert_eq!(r.code, 2);
    assert_eq!(r.stdout, "");
    assert!(r.stderr.starts_with("gliffherdr: "), "{}", r.stderr);
    assert!(r.stderr.contains("unknown field `colour`"), "{}", r.stderr);
    assert!(
        r.stderr.contains("line 3"),
        "toml's message carries the line: {}",
        r.stderr
    );
    assert!(r.stderr.contains("config.toml"), "{}", r.stderr);
}

#[test]
fn unknown_keys_stop_list_and_check_with_exit_2() {
    let w = World::new();
    w.write_config("colour = \"blue\"\n");
    for args in [
        &["list"][..],
        &["list", "--json"],
        &["check", "--all"],
        &["check", "j@h"],
    ] {
        let r = w.run(args);
        assert_eq!(r.code, 2, "{args:?}");
        assert!(
            r.stderr.contains("unknown field `colour`"),
            "{args:?}: {}",
            r.stderr
        );
        assert_eq!(r.stdout, "");
    }
    assert_eq!(w.ssh_calls().len(), 0, "nothing ran");
}

#[test]
fn invalid_values_are_exit_2_with_the_reason() {
    let w = World::new();
    let cases = [
        (
            "[[machine]]\ntarget = \"-oProxyCommand=x\"\n",
            "invalid target",
        ),
        (
            "[[machine]]\ntarget = \"h\"\noutput = \"a b\"\n",
            "not a screen name",
        ),
        ("[check]\nssh_timeout_secs = 0\n", "ssh_timeout_secs"),
        ("[check]\nssh_timeout_secs = 61\n", "ssh_timeout_secs"),
        ("[check]\nfresh_for_secs = 0\n", "fresh_for_secs"),
        ("[connect]\nmode = \"sideways\"\n", "sideways"),
        ("[connect\n", "line 1"),
    ];
    for (text, needle) in cases {
        w.write_config(text);
        let r = w.run(&["config", "check"]);
        assert_eq!(r.code, 2, "{text}");
        assert!(r.stderr.contains(needle), "{text}: {}", r.stderr);
    }
}

#[test]
fn a_full_config_is_accepted() {
    let w = World::new();
    w.write_config(
        "[connect]\nmode = \"headless\"\ngliff_args = [\"--no-clipboard\"]\nserver_bin = \"\"\n\
         [sources]\nherdr = false\nrecent = false\ngliff = false\nssh_config = false\ntailscale = false\nhide = [\"x\"]\n\
         [check]\non_startup = false\nfresh_for_secs = 5\nssh_timeout_secs = 1\n\
         [[machine]]\ntarget = \"j@h\"\nlabel = \"desk\"\nmode = \"mirror\"\noutput = \"DP-1\"\nserver_bin = \"s\"\ngliff_args = []\n",
    );
    assert_eq!(w.run(&["config", "check"]).stdout, "ok\n");
    let r = w.run(&["list", "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.contains("\"j@h\""));
}
