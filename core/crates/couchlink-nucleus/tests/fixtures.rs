//! Tests over the saved hub fixtures: `tests/fixtures/allhandlers_sample.json` (20 rows of the
//! catalog) and nine full handlers (`handler.json`, `screenshots.json`, `handler.js` each).

use couchlink_nucleus::{Draft, HubGame, list, parse_screenshots, to_recipe, try_list};
use serde_json::Value;
use std::fs;

const GAMES: &[&str] = &[
    "borderlands-2",
    "dark-souls-3-seamless",
    "fallout-new-vegas-nvmp",
    "left-4-dead-2",
    "monster-hunter-world",
    "palworld",
    "risk-of-rain-2",
    "subnautica-nitrox",
    "terraria",
];

/// The hub handlers are other people's scripts, so they are kept on the developer's PC and never committed
/// (`.gitignore`). Without them these tests say so and pass.
fn have_fixtures() -> bool {
    let ok = std::path::Path::new(&dir()).join("handlers").is_dir();
    if !ok {
        println!("SKIP (hub handler fixtures are local only)");
    }
    ok
}

fn dir() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn read(game: &str, file: &str) -> String {
    fs::read_to_string(format!("{}/handlers/{game}/{file}", dir())).unwrap()
}

fn sample() -> Vec<HubGame> {
    list(&fs::read_to_string(format!("{}/allhandlers_sample.json", dir())).unwrap())
}

fn draft(game: &str) -> Draft {
    to_recipe(&read(game, "handler.js")).unwrap()
}

fn has_note(d: &Draft, needle: &str) -> bool {
    d.notes.iter().any(|n| n.contains(needle))
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect()
}

// ---------------------------------------------------------------------------------------------
// catalog
// ---------------------------------------------------------------------------------------------

#[test]
fn catalog_sample_lists_twenty_games_with_art() {
    if !have_fixtures() {
        return;
    }
    let games = sample();
    assert_eq!(games.len(), 20);
    for g in &games {
        assert!(!g.id.is_empty() && !g.title.is_empty() && !g.game_name.is_empty(), "{g:?}");
        assert!(g.igdb_id.is_some(), "{} has no igdb id", g.title);
        let cover = g.cover_url.as_deref().expect("cover");
        assert!(cover.starts_with("https://images.igdb.com/igdb/image/upload/t_cover_big/"), "{cover}");
        assert!(cover.ends_with(".jpg"));
        assert!(g.screenshot_urls.is_empty(), "the catalog carries no screenshots");
        assert!(g.handler_version >= 1);
        assert!(g.max_players >= 1);
    }
}

#[test]
fn catalog_row_fields_for_subnautica() {
    if !have_fixtures() {
        return;
    }
    let games = sample();
    let g = games.iter().find(|g| g.id == "SXxys76eLF6BC36PF").expect("Subnautica Nitrox in the sample");
    assert_eq!(g.title, "Subnautica Nitrox");
    assert_eq!(g.game_name, "Subnautica");
    assert_eq!(g.igdb_id, Some(9254));
    assert_eq!(g.cover_url.as_deref(), Some("https://images.igdb.com/igdb/image/upload/t_cover_big/co1iqw.jpg"));
    assert!(g.downloads > 10_000);
    assert!(g.stars > 0);
    assert!(g.verified);
    assert!(g.handler_version >= 8);
    assert_eq!(g.owner_name, "Talos91");
    assert!(g.genres.iter().any(|x| x == "Adventure"));
    assert!(g.controllers && g.keyboard_mouse);
    assert_eq!(g.source(), "splitscreenme:SXxys76eLF6BC36PF");
    assert_eq!(g.handler_url(), "https://hub.splitscreen.me/api/v1/handler/SXxys76eLF6BC36PF");
    assert_eq!(g.screenshots_url(), "https://hub.splitscreen.me/api/v1/screenshots/SXxys76eLF6BC36PF");
    assert!(g.package_url().starts_with("https://hub.splitscreen.me/cdn/storage/packages/"));
    assert!(g.package_url().contains(&format!("/original/handler-SXxys76eLF6BC36PF-v{}.nc", g.handler_version)));
    assert_eq!(
        g.cover_url_sized("t_720p").as_deref(),
        Some("https://images.igdb.com/igdb/image/upload/t_720p/co1iqw.jpg")
    );
}

#[test]
fn screenshots_response_becomes_full_urls() {
    if !have_fixtures() {
        return;
    }
    let json = read("subnautica-nitrox", "screenshots.json");
    let urls = parse_screenshots(&json, "t_screenshot_big");
    assert!(urls.len() >= 5);
    assert_eq!(urls[0], "https://images.igdb.com/igdb/image/upload/t_screenshot_big/sc68wb.jpg");
    assert!(urls.iter().all(|u| u.starts_with("https://") && !u.contains("t_thumb")));

    let games = try_list(&read("subnautica-nitrox", "handler.json")).unwrap();
    assert_eq!(games.len(), 1);
    let g = games.into_iter().next().unwrap().with_screenshots(&json);
    assert_eq!(g.screenshot_urls, urls);
}

#[test]
fn screenshots_without_image_id_fall_back_to_the_url() {
    if !have_fixtures() {
        return;
    }
    let json = r#"{"screenshots":[{"url":"//images.igdb.com/igdb/image/upload/t_thumb/zz.jpg"},{"nothing":1}]}"#;
    assert_eq!(
        parse_screenshots(json, "t_1080p"),
        vec!["https://images.igdb.com/igdb/image/upload/t_1080p/zz.jpg".to_string()]
    );
}

#[test]
fn screenshots_error_response_is_empty() {
    if !have_fixtures() {
        return;
    }
    assert!(parse_screenshots(r#"{"error":"Incorrect handlerId"}"#, "t_thumb").is_empty());
    assert!(parse_screenshots("not json", "t_thumb").is_empty());
}

#[test]
fn handler_endpoint_row_matches_catalog_row() {
    if !have_fixtures() {
        return;
    }
    let sample = sample();
    let mut matched = 0;
    for game in GAMES {
        let from_handler: Vec<HubGame> = try_list(&read(game, "handler.json")).unwrap();
        assert_eq!(from_handler.len(), 1, "{game}");
        let h = &from_handler[0];
        if let Some(s) = sample.iter().find(|s| s.id == h.id) {
            matched += 1;
            assert_eq!(s.title, h.title);
            assert_eq!(s.game_name, h.game_name);
            assert_eq!(s.igdb_id, h.igdb_id);
            assert_eq!(s.cover_url, h.cover_url);
            assert_eq!(s.package_id, h.package_id);
        }
    }
    assert!(matched >= 8, "the sample should hold the fixture games");
}

#[test]
fn list_tolerates_bad_input() {
    if !have_fixtures() {
        return;
    }
    assert!(list("").is_empty());
    assert!(list("not json").is_empty());
    assert!(list(r#"{"Handlers":[]}"#).is_empty());
    // a malformed row and a private row are skipped, a bare array works
    let json = r#"[{"nope":1},{"_id":"p","title":"Hidden","private":true},{"_id":"a","title":"A","currentVersion":"3","downloadCount":"12"}]"#;
    let g = list(json);
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].id, "a");
    assert_eq!(g[0].game_name, "A");
    assert_eq!(g[0].handler_version, 3);
    assert_eq!(g[0].downloads, 12);
    assert!(g[0].cover_url.is_none());
    assert!(try_list("{").is_err());
}

// ---------------------------------------------------------------------------------------------
// every fixture
// ---------------------------------------------------------------------------------------------

#[test]
fn every_fixture_yields_a_well_formed_recipe() {
    if !have_fixtures() {
        return;
    }
    for game in GAMES {
        let d = draft(game);
        let r = &d.recipe;
        let hub: Vec<HubGame> = try_list(&read(game, "handler.json")).unwrap();
        assert_eq!(r["schema"], 1, "{game}");
        assert_eq!(r["source"], format!("splitscreenme:{}", hub[0].id), "{game}");
        assert_eq!(r["mode"], "instances");
        assert_eq!(r["present"], "window");
        let id = r["id"].as_str().unwrap();
        assert!(
            id.starts_with("splitscreenme-") && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{id}"
        );
        assert!(matches!(r["steam"].as_str().unwrap(), "none" | "shared" | "goldberg"));
        let players = r["players"].as_array().unwrap();
        assert!(players[0].as_u64().unwrap() <= players[1].as_u64().unwrap());
        assert!(r["match"]["exe"].is_string() && r["match"]["steamAppId"].is_u64(), "{game}");
        assert!(r["launch"]["exe"].is_string());
        assert!(r["files"]["copy"].is_array() && r["files"]["saves"].is_array());
        assert!(r["registry"].is_array() && r["locks"].is_array());
        for k in ["xinput", "dinput", "rawinput"] {
            assert!(r["input"][k].is_boolean(), "{game} input.{k}");
        }
        assert!(matches!(r["input"]["keyboard"].as_str().unwrap(), "none" | "single" | "multi"));
        assert!(r["window"]["settle_ms"].as_u64().unwrap() >= 1000);
        assert!((0.05..=1.0).contains(&d.confidence), "{game} {}", d.confidence);
        // the recipe must survive a JSON round trip
        let text = serde_json::to_string(r).unwrap();
        assert_eq!(&serde_json::from_str::<Value>(&text).unwrap(), r);
        // no template placeholder escapes into the match block
        assert!(!r["match"]["exe"].as_str().unwrap().contains('$'));
        // notes are one line each and never empty
        assert!(d.notes.iter().all(|n| !n.trim().is_empty() && !n.contains('\n')), "{game}");
    }
}

#[test]
fn the_same_script_through_a_bundle_adds_hub_metadata() {
    if !have_fixtures() {
        return;
    }
    for game in GAMES {
        let bundle = serde_json::json!({
            "handler": serde_json::from_str::<Value>(&read(game, "handler.json")).unwrap(),
            "script": read(game, "handler.js"),
        });
        let with = to_recipe(&bundle.to_string()).unwrap();
        let plain = draft(game);
        assert_eq!(with.confidence, plain.confidence, "{game}");
        assert_eq!(with.recipe["source"], plain.recipe["source"]);
        assert!(with.recipe["meta"]["igdb_id"].is_u64(), "{game}");
        assert!(with.recipe["meta"]["hub_title"].is_string());
    }
}

#[test]
fn metadata_only_gives_a_low_confidence_skeleton() {
    if !have_fixtures() {
        return;
    }
    let d = to_recipe(&read("subnautica-nitrox", "handler.json")).unwrap();
    assert!(d.confidence <= 0.15, "{}", d.confidence);
    assert_eq!(d.recipe["source"], "splitscreenme:SXxys76eLF6BC36PF");
    assert_eq!(d.recipe["game"], "Subnautica");
    assert_eq!(d.recipe["players"], serde_json::json!([2, 4]));
    assert!(d.recipe["match"].get("exe").is_none());
    assert!(has_note(&d, "Only hub metadata"));
    assert!(has_note(&d, "ExecutableName"));
}

#[test]
fn bad_inputs_are_errors() {
    if !have_fixtures() {
        return;
    }
    use couchlink_nucleus::Error;
    assert_eq!(to_recipe("").unwrap_err(), Error::Empty);
    assert_eq!(to_recipe("  \n").unwrap_err(), Error::Empty);
    assert!(matches!(to_recipe("{ not json").unwrap_err(), Error::Json(_)));
    assert_eq!(to_recipe(r#"{"hello":"world"}"#).unwrap_err(), Error::NotAHandler);
    assert!(!Error::Empty.to_string().is_empty());
}

#[test]
fn confidence_separates_simple_handlers_from_scripted_ones() {
    if !have_fixtures() {
        return;
    }
    let c = |g: &str| draft(g).confidence;
    // few unsupported bits
    assert!(c("terraria") >= 0.8, "terraria {}", c("terraria"));
    assert!(c("risk-of-rain-2") >= 0.8, "risk-of-rain-2 {}", c("risk-of-rain-2"));
    // a person has real work to do
    for g in [
        "subnautica-nitrox",
        "monster-hunter-world",
        "dark-souls-3-seamless",
        "palworld",
        "left-4-dead-2",
        "fallout-new-vegas-nvmp",
    ] {
        assert!(c(g) < 0.5, "{g} {}", c(g));
    }
    let bl2 = c("borderlands-2");
    assert!((0.5..0.8).contains(&bl2), "borderlands-2 {bl2}");
}

// ---------------------------------------------------------------------------------------------
// per-game mapping
// ---------------------------------------------------------------------------------------------

#[test]
fn subnautica_nitrox() {
    if !have_fixtures() {
        return;
    }
    let d = draft("subnautica-nitrox");
    let r = &d.recipe;
    assert_eq!(r["game"], "Subnautica Nitrox");
    assert_eq!(r["match"]["steamAppId"], 264710);
    assert_eq!(r["match"]["exe"], "Subnautica.exe");
    assert_eq!(r["players"], serde_json::json!([2, 4]));
    assert_eq!(r["steam"], "shared");
    assert_eq!(r["files"]["link"], "none"); // SymlinkGame = false
    assert_eq!(r["launch"]["env"]["SteamAppId"], "264710");
    assert_eq!(strings(&r["launch"]["kill_on_exit"]), ["Nitrox.Launcher", "NitroxServer-Subnautica"]);
    assert_eq!(r["input"]["xinput"], true);
    assert_eq!(r["input"]["dinput"], false);
    assert_eq!(r["input"]["keyboard"], "multi");
    assert_eq!(r["hook"]["clip_cursor"], true);
    assert_eq!(r["hook"]["focus"], false);
    assert_eq!(r["hook"]["block_messages"], serde_json::json!([8, 675, 673]));
    assert_eq!(r["window"]["settle_ms"], 30000);
    assert_eq!(r["window"]["grab_delay_ms"], 15000);
    assert_eq!(r["window"]["topmost"], true);
    assert_eq!(r["window"]["title"], "Subnautica");
    // five Unity PlayerPrefs registry values, two of them templated
    let reg = r["registry"].as_array().unwrap();
    assert_eq!(reg.len(), 5);
    assert_eq!(reg[0]["key"], "HKCU\\SOFTWARE\\Unknown Worlds\\Subnautica");
    assert_eq!(reg[0]["value"], 3);
    let values: Vec<&Value> = reg.iter().map(|e| &e["value"]).collect();
    assert!(values.contains(&&Value::from("$WIDTH")) && values.contains(&&Value::from("$HEIGHT")));
    // what the recipe cannot do
    assert!(has_note(&d, "Nitrox.Launcher.exe"), "{:?}", d.notes);
    assert!(has_note(&d, "NitroxServer-Subnautica.exe"));
    assert!(has_note(&d, "administrator"));
    assert!(has_note(&d, "outside program"));
    assert!(has_note(&d, "pick the game process"));
    assert!(r["instructions"].as_str().unwrap().contains("Nitrox"));
}

#[test]
fn monster_hunter_world_goldberg_and_steamless() {
    if !have_fixtures() {
        return;
    }
    let d = draft("monster-hunter-world");
    let r = &d.recipe;
    assert_eq!(r["match"]["steamAppId"], 582010);
    assert_eq!(r["match"]["exe"], "MonsterHunterWorld.exe");
    assert_eq!(r["steam"], "goldberg");
    assert_eq!(strings(&r["locks"]), ["sMhMain"]);
    assert_eq!(r["files"]["link"], "all");
    let copy = strings(&r["files"]["copy"]);
    assert!(copy.contains(&"config.ini") && copy.contains(&"nvngx_dlss.dll"));
    // Goldberg bookkeeping and input shims are not exclusions in Couchlink
    let excl = strings(&r["files"]["exclude_files"]);
    assert!(excl.contains(&"lazy.ini"));
    assert!(!excl.contains(&"steam_appid.txt") && !excl.contains(&"dinput8.dll") && !excl.contains(&"xinput9_1_0.dll"));
    assert!(r["files"]["saves"].as_array().unwrap().is_empty()); // the only path is a Goldberg save folder
    assert_eq!(r["window"]["title"], "MONSTER HUNTER: WORLD(421810)");
    assert_eq!(r["window"]["title_partial"], true);
    assert_eq!(r["hook"]["set_window_pos"], true);
    assert_eq!(r["hook"]["cursor"], true);
    assert_eq!(r["hook"]["focus"], true); // Hook.ForceFocus
    assert!(has_note(&d, "Steamless"));
    assert!(has_note(&d, "experimental Goldberg"));
    assert!(has_note(&d, "INI settings"));
    assert!(has_note(&d, "Game.FakeFocus is set under a condition"));
    assert!(has_note(&d, "Lazy_Aspect_Fix"));
    assert!(has_note(&d, "6 user option"));
}

#[test]
fn fallout_new_vegas_launches_a_different_exe() {
    if !have_fixtures() {
        return;
    }
    let d = draft("fallout-new-vegas-nvmp");
    let r = &d.recipe;
    assert_eq!(r["game"], "NVMP");
    assert_eq!(r["match"]["exe"], "FalloutNV.exe");
    assert_eq!(r["launch"]["exe"], "nvmp_start.exe");
    assert!(r["launch"]["args"].as_str().unwrap().starts_with("localhost 27015 22380 null null \"CaravanPack.esm,"));
    assert_eq!(strings(&r["locks"]), ["FalloutNVGameMutex"]);
    assert_eq!(r["input"]["keyboard"], "single"); // SupportsKeyboard only
    let saves = strings(&r["files"]["saves"]);
    assert!(saves.contains(&"$USERPROFILE/AppData/Local/FalloutNV"));
    assert!(saves.contains(&"$USERPROFILE/Documents/My Games/FalloutNV"));
    assert_eq!(strings(&r["files"]["exclude_files"]), ["nvse_config.ini"]);
    assert_eq!(r["input"]["dinput"], false); // DInputForceDisable
    assert!(has_note(&d, "instead of the game exe (FalloutNV.exe)"));
    assert!(has_note(&d, "nvserver.bat"));
    assert!(has_note(&d, "35 place(s)"));
}

#[test]
fn left_4_dead_2_args_and_mutex() {
    if !have_fixtures() {
        return;
    }
    let d = draft("left-4-dead-2");
    let r = &d.recipe;
    assert_eq!(r["match"]["steamAppId"], 550);
    assert_eq!(r["steam"], "goldberg");
    assert_eq!(strings(&r["locks"]), ["hl2_singleton_mutex"]);
    // the gamepad-instance branch, templated
    assert_eq!(
        r["launch"]["args"],
        "-windowed -noborder -nointro -novid -steam +net_graph 0 -w $WIDTH -h $HEIGHT -x $X -y $Y"
    );
    assert!(has_note(&d, "2 variant(s)"));
    assert!(has_note(&d, "+cl_mouselook 1"));
    assert_eq!(r["players"], serde_json::json!([2, 4]));
    assert_eq!(strings(&r["files"]["copy"]), ["config.cfg", "video.txt", "debug.log", "360controller.cfg"]);
    assert_eq!(r["input"]["rawinput"], true);
    assert_eq!(r["window"]["title"], "Left 4 Dead 2 - Direct3D 9");
}

#[test]
fn borderlands_2_binaries_folder_and_dynamic_args() {
    if !have_fixtures() {
        return;
    }
    let d = draft("borderlands-2");
    let r = &d.recipe;
    assert_eq!(r["match"]["exe"], "Borderlands2.exe");
    assert_eq!(r["launch"]["exe"], "Binaries/Win32/Borderlands2.exe");
    assert_eq!(r["launch"]["workdir"], "Binaries/Win32");
    assert_eq!(r["launch"]["args"], ""); // built from a user option: left empty
    assert!(has_note(&d, "script variables"));
    assert_eq!(r["steam"], "goldberg");
    assert_eq!(
        r["files"]["saves"],
        serde_json::json!(["$USERPROFILE/Documents/My Games/Borderlands 2/WillowGame/SaveData"])
    );
    assert!(has_note(&d, "4 user option"));
}

#[test]
fn risk_of_rain_2_unity_registry_and_args() {
    if !have_fixtures() {
        return;
    }
    let d = draft("risk-of-rain-2");
    let r = &d.recipe;
    assert_eq!(r["launch"]["args"], "-screen-fullscreen 0 -popupwindow -screen-width $WIDTH -screen-height $HEIGHT");
    assert_eq!(r["registry"].as_array().unwrap().len(), 4);
    assert_eq!(r["registry"][1]["name"], "Screenmanager Resolution Height_h2627697771");
    assert_eq!(r["registry"][1]["value"], "$HEIGHT");
    assert_eq!(r["hook"]["focus"], true);
    assert_eq!(r["hook"]["focus_interval_ms"], 7000);
    assert_eq!(r["steam"], "goldberg");
    assert!(has_note(&d, "asks the user before opening each next instance"));
    // the Goldberg files this script writes are not reported as unsupported
    assert!(!d.notes.iter().any(|n| n.contains("writes files")), "{:?}", d.notes);
}

#[test]
fn palworld_unreal_with_epic_emulation() {
    if !have_fixtures() {
        return;
    }
    let d = draft("palworld");
    let r = &d.recipe;
    assert_eq!(r["match"]["exe"], "Palworld-Win64-Shipping.exe");
    assert_eq!(r["launch"]["exe"], "Pal/Binaries/Win64/Palworld-Win64-Shipping.exe");
    assert_eq!(r["launch"]["args"], "-windowed -AlwaysFocus -nosplash -ResX=$WIDTH -ResY=$HEIGHT");
    assert_eq!(strings(&r["launch"]["kill_on_exit"]), ["PalServer", "PalServer-Win64-Test-Cmd"]);
    assert_eq!(r["steam"], "goldberg");
    assert_eq!(
        strings(&r["files"]["saves"]),
        ["$USERPROFILE/AppData/Local/Pal/Saved/Config/Windows", "$USERPROFILE/AppData/Local/Pal/Saved/SaveGames"]
    );
    assert_eq!(r["hook"]["focus_interval_ms"], 10000);
    assert!(has_note(&d, "Epic Online Services"));
    assert!(has_note(&d, "server.bat"));
}

#[test]
fn dark_souls_3_launcher_and_steamless() {
    if !have_fixtures() {
        return;
    }
    let d = draft("dark-souls-3-seamless");
    let r = &d.recipe;
    assert_eq!(r["match"]["exe"], "DarkSoulsIII.exe");
    assert_eq!(r["launch"]["exe"], "ds3sc_launcher.exe");
    assert_eq!(strings(&r["locks"]), ["DarkSoulsIIIMutex"]);
    assert_eq!(r["steam"], "shared");
    assert_eq!(strings(&r["files"]["saves"]), ["$USERPROFILE/AppData/Roaming/DarkSoulsIII"]);
    // steam bookkeeping and the xinput shim are not exclusions; dxgi.dll is
    let excl = strings(&r["files"]["exclude_files"]);
    assert!(excl.contains(&"dxgi.dll") && excl.contains(&"winmm.dll"));
    assert!(!excl.contains(&"steam_appid.txt") && !excl.contains(&"xinput1_3.dll"));
    assert!(has_note(&d, "launcher ds3sc_launcher.exe"));
    assert!(has_note(&d, "Steamless"));
    assert!(has_note(&d, "Game.HookFocus is set under a condition"));
    assert!(has_note(&d, "patches files in place"));
    assert!(has_note(&d, "XML"));
}

#[test]
fn terraria_documents_saves_and_no_goldberg() {
    if !have_fixtures() {
        return;
    }
    let d = draft("terraria");
    let r = &d.recipe;
    assert_eq!(r["steam"], "shared");
    assert_eq!(r["match"]["steamAppId"], 105600);
    assert_eq!(r["players"], serde_json::json!([2, 4]));
    // Documents config and the Players folder inside it collapse to the outer folder
    assert_eq!(strings(&r["files"]["saves"]), ["$DOCUMENTS/My Games/Terraria"]);
    assert_eq!(strings(&r["files"]["copy"]), ["steam_api.dll"]);
    assert_eq!(strings(&r["files"]["exclude_files"]), ["config.json"]);
    assert_eq!(r["hook"]["set_window_pos"], true);
    assert_eq!(r["hook"]["focus"], true);
    assert_eq!(r["window"]["settle_ms"], 40000);
    assert!(has_note(&d, "1 user option"));
}

// ---------------------------------------------------------------------------------------------
// odd and hostile JavaScript
// ---------------------------------------------------------------------------------------------

fn recipe_of(js: &str) -> Draft {
    to_recipe(js).unwrap()
}

#[test]
fn tolerates_missing_semicolons_comments_and_crlf() {
    if !have_fixtures() {
        return;
    }
    let js = "Hub.Handler.Id = \"abc123\"\r\nGame.ExecutableName = \"a.exe\" // trailing { comment \"\r\n/* block\r\n with ; and \" quotes */\r\nGame.SteamID = '7'\r\nGame.MaxPlayers = 3\r\nGame.KillMutex = [\"m1\",\r\n \"m2\", ]\r\n";
    let d = recipe_of(js);
    assert_eq!(d.recipe["match"]["exe"], "a.exe");
    assert_eq!(d.recipe["match"]["steamAppId"], 7);
    assert_eq!(d.recipe["players"], serde_json::json!([2, 3]));
    assert_eq!(strings(&d.recipe["locks"]), ["m1", "m2"]);
    assert_eq!(d.recipe["source"], "splitscreenme:abc123");
}

#[test]
fn tolerates_bom_unterminated_strings_and_junk() {
    if !have_fixtures() {
        return;
    }
    let js = "\u{feff}Game.ExecutableName = \"ok.exe\";\nGame.Description = \"never closed;\nGame.MaxPlayers = 4;\n@@@ ### ;;; }}} )))\nGame.SteamID = \"12\";\n";
    let d = recipe_of(js);
    assert_eq!(d.recipe["match"]["exe"], "ok.exe");
    assert_eq!(d.recipe["players"], serde_json::json!([2, 4]));
    assert_eq!(d.recipe["match"]["steamAppId"], 12);
}

#[test]
fn tolerates_unbalanced_braces_and_brackets() {
    if !have_fixtures() {
        return;
    }
    let js = "Game.ExecutableName = \"x.exe\";\nGame.Play = function() { if (a) { Context.EditRegKey(\"HKEY_CURRENT_USER\", \"S\\\\K\", \"n\", 1, Nucleus.RegType.DWord);\n";
    let d = recipe_of(js);
    assert_eq!(d.recipe["match"]["exe"], "x.exe");
    // inside an unclosed `if`: conditional, so not a registry entry
    assert_eq!(d.recipe["registry"].as_array().unwrap().len(), 0);
    assert!(has_note(&d, "set under a condition"));
    let d = recipe_of("Game.ExecutableName = [\"unclosed.exe\", ");
    assert!(d.confidence <= 0.2); // exe not understood
}

#[test]
fn regex_template_ternary_and_object_values_do_not_derail_parsing() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
var re = /ab+c[/]\/"/gi;
var t = `template ${x} text`;
Game.GameName = Context.Options["a"] ? "A" : "B";
Game.Weird = { a: 1, b: [2, 3], c: function () { return "}"; } };
Game.ExecutableName = "after.exe";
Game.MaxPlayers = (1 + 2) * 2;
Game.SteamID = 0x10;
"#;
    let d = recipe_of(js);
    assert_eq!(d.recipe["match"]["exe"], "after.exe");
    assert_eq!(d.recipe["match"]["steamAppId"], 16);
    assert_eq!(d.recipe["game"], "Unknown game"); // ternary is not evaluated
    assert_eq!(d.recipe["players"], serde_json::json!([2, 4]));
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    if !have_fixtures() {
        return;
    }
    let mut js = String::from("Game.ExecutableName = \"deep.exe\";\nGame.X = ");
    js.push_str(&"(".repeat(20_000));
    js.push('1');
    js.push_str(&")".repeat(20_000));
    js.push_str(";\nGame.Play = function() {");
    js.push_str(&"if (a) {".repeat(20_000));
    js.push_str(&"}".repeat(20_000));
    js.push_str("};\nGame.SteamID = \"5\";");
    let d = recipe_of(&js);
    assert_eq!(d.recipe["match"]["exe"], "deep.exe");
}

#[test]
fn kill_mutex_type_prefix_and_flags() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
Game.ExecutableName = "m.exe";
Game.KillMutex = ["Type:Event|EvName", "Type:Mutant|MutName", "Plain", "Plain"];
Game.PartialMutexSearch = true;
Game.ProtoInput.RenameHandles = ["Extra"];
Game.ProtoInput.RenameNamedPipes = ["pipe1"];
"#;
    let d = recipe_of(js);
    assert_eq!(strings(&d.recipe["locks"]), ["EvName", "MutName", "Plain", "Extra"]);
    assert!(has_note(&d, "is a Event"));
    assert!(has_note(&d, "partially"));
    assert!(has_note(&d, "pipe1"));
}

#[test]
fn start_arguments_templates_and_appends() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
Game.ExecutableName = "m.exe";
Game.StartArguments = "-base";
Game.Play = function () {
  Context.StartArguments += " -w " + Context.Width + " -h " + Context.Height;
  Context.StartArguments += " -x " + Context.PosX + " -y " + Context.PosY + " -name " + Context.Nickname;
};
"#;
    let d = recipe_of(js);
    assert_eq!(d.recipe["launch"]["args"], "-base -w $WIDTH -h $HEIGHT -x $X -y $Y -name $PLAYER");
    assert!(d.notes.iter().all(|n| !n.contains("arguments")), "{:?}", d.notes);
}

#[test]
fn unknown_references_in_arguments_are_dropped_and_noted() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
Game.ExecutableName = "m.exe";
Game.Play = function () {
  var level = Context.Options["lvl"];
  Context.StartArguments = "-map " + level + " -w " + Context.Width;
};
"#;
    let d = recipe_of(js);
    assert_eq!(d.recipe["launch"]["args"], "-map  -w $WIDTH");
    assert!(has_note(&d, "built by script logic"));
    assert!(d.confidence < 0.9);
}

#[test]
fn registry_edits_are_unconditional_only() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
Game.ExecutableName = "m.exe";
Game.Play = function () {
  Context.EditRegKey("HKEY_CURRENT_USER", "SOFTWARE\\Co\\G", "Width", Context.Width, Nucleus.RegType.DWord);
  Context.EditRegKey("HKEY_LOCAL_MACHINE", "SOFTWARE\\Co\\G", "Name", "x", Nucleus.RegType.String);
  if (Context.IsKeyboardPlayer) {
    Context.EditRegKey("HKEY_CURRENT_USER", "SOFTWARE\\Co\\G", "Kb", 1, Nucleus.RegType.DWord);
  }
  Context.EditRegKey("HKEY_CURRENT_USER", "SOFTWARE\\Co\\G", "Computed", Context.Options["q"], Nucleus.RegType.DWord);
};
"#;
    let d = recipe_of(js);
    let reg = d.recipe["registry"].as_array().unwrap();
    assert_eq!(reg.len(), 2);
    assert_eq!(reg[0], serde_json::json!({"key": "HKCU\\SOFTWARE\\Co\\G", "name": "Width", "value": "$WIDTH"}));
    assert_eq!(reg[1], serde_json::json!({"key": "HKLM\\SOFTWARE\\Co\\G", "name": "Name", "value": "x"}));
    assert!(has_note(&d, "\"Kb\" is set under a condition"));
    assert!(has_note(&d, "\"Computed\" is value computed by script"));
}

#[test]
fn flags_without_a_recipe_equivalent_lower_confidence_and_say_so() {
    if !have_fixtures() {
        return;
    }
    let js = r#"
Game.ExecutableName = "m.exe";
Game.RequiresAdmin = true;
Game.UseNemirtingasEpicEmu = true;
Game.UseEACBypass = true;
Game.HexEditExe = ["a|b"];
Game.CMDBatchBefore = ["echo hi"];
Game.OnStop = function () { };
"#;
    let d = recipe_of(js);
    assert!(d.confidence < 0.4, "{}", d.confidence);
    for needle in ["administrator", "Epic", "Easy Anti-Cheat", "hex-edits the exe", "before launch", "Game.OnStop"] {
        assert!(has_note(&d, needle), "missing note {needle}: {:?}", d.notes);
    }
}

#[test]
fn input_flags_follow_handler_fields() {
    if !have_fixtures() {
        return;
    }
    let d = recipe_of(
        r#"Game.ExecutableName = "m.exe";
Game.Hook.XInputEnabled = false;
Game.XInputPlusDll = ["xinput1_3.dll"];
Game.Hook.DInputEnabled = true;
Game.SupportsKeyboard = true;
Game.ProtoInput.RegisterRawInputHook = true;
Game.FakeFocus = true;
Game.FakeFocusInterval = 250;
Game.HookSetCursorPos = true;
Game.DontRemoveBorders = true;
Game.HideTaskbar = true;
"#,
    );
    let r = &d.recipe;
    assert_eq!(r["input"], serde_json::json!({"xinput": true, "dinput": true, "rawinput": true, "keyboard": "single"}));
    assert_eq!(r["hook"]["focus"], true);
    assert_eq!(r["hook"]["focus_interval_ms"], 250);
    assert_eq!(r["hook"]["cursor"], true);
    assert_eq!(r["window"]["borderless"], false);
    assert_eq!(r["window"]["hide_taskbar"], true);
}

#[test]
fn no_steam_id_means_steam_none_and_goldberg_without_id_is_flagged() {
    if !have_fixtures() {
        return;
    }
    let d = recipe_of("Game.ExecutableName = \"free.exe\"; Game.MaxPlayers = 1;");
    assert_eq!(d.recipe["steam"], "none");
    assert_eq!(d.recipe["players"], serde_json::json!([1, 1]));
    assert!(d.recipe["match"].get("steamAppId").is_none());
    let d = recipe_of("Game.ExecutableName = \"free.exe\"; Game.UseGoldberg = true;");
    assert_eq!(d.recipe["steam"], "goldberg");
    assert!(has_note(&d, "no Steam app id"));
}

#[test]
fn no_source_id_is_reported() {
    if !have_fixtures() {
        return;
    }
    let d = recipe_of("Game.ExecutableName = \"free.exe\";");
    assert_eq!(d.recipe["source"], "splitscreenme:unknown");
    assert!(has_note(&d, "Hub.Handler.Id"));
}
