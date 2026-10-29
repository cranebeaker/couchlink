# couchlink-steam

The Steam layer of Couchlink: finds Steam libraries and installed games, and turns each copy of a game into its
own Steam player with gbe_fork (Goldberg Steam emulator fork, pinned release, LGPL-3, shipped only as unmodified
separate files). Contract: `docs/launch.md` ("The Steam ladder", "Steam layer details").

| Module | What |
|---|---|
| `library`, `vdf` | read-only scan of Steam libraries and appmanifests |
| `goldberg` | pinned download, SHA-256 check, extraction; `ensure_goldberg`, `ensure_goldberg_steamclient` |
| `prepare` | `prepare`, `prepare_with`, `prepare_full`: per-seat setup in a hardlinked copy, `Undo` records |
| `options` | `GoldbergOptions`: the typed extra gbe settings a recipe may ask for |
| `interfaces` | `steam_interfaces.txt` from the original `steam_api(64).dll` (port of gbe's `generate_interfaces`) |
| `dlc` | the DLC list, installed DLC only |
| `steamclient` | ColdClientLoader mode |
| `accounts` | opt-in second real Steam account |

Rules every write follows: never write into an existing file (it may be a hardlink shared with the base install;
the old file is moved aside and a new one created), journal every change as an `Undo`, roll back on failure.
Couchlink never touches antivirus settings; a Defender block is reported as `SteamError::BlockedByAntivirus`.

Run the tests with `cargo test -p couchlink-steam`. They use fake archives and fake DLL bytes in temp folders; the
one test that downloads the real archive is `#[ignore]`.

## EOS (Epic Online Services) stand-in: design note, not built

Some games talk to Epic Online Services next to Steam (Epic login, lobbies, sessions, P2P). Goldberg does not
answer EOS, so copies of such a game on one PC cannot find each other. The refined recipes that need an EOS stand-in
(`needs` has `epic-emu`):

| Recipe | Steam app id | Note |
|---|---|---|
| Palworld | 1623730 | window size also needs an ini edit |
| Remnant: From the Ashes | 617290 | also a five-item DLC list |
| Backrooms: Escape Together | 2141730 | per-seat json with its own id and name |
| Escape The Backrooms | 1943950 | recipe field `epic_emu: "nemirtingas-eos"` |
| Killing Floor 2 | 232090 | `epic_emu: "nemirtingas-eos"`, plus items/achievements json for gbe |

What a stand-in for local copies would take:

1. **A replacement `EOSSDK-Win64-Shipping.dll` per copy**, swapped in exactly like `steam_api64.dll` (move the
   original aside, create a new file, journal an `Undo`). It must export the same C ABI: `EOS_Initialize`,
   `EOS_Platform_Create`, the `EOS_Platform_Get*Interface` getters and the interface functions the games call
   (Auth, Connect, UserInfo, Friends, Presence, Lobby, Sessions, P2P, Ecom offers, Achievements, Stats, Metrics,
   Platform tick and callbacks). Structs are versioned, so each function must accept the versions the game was built with.
2. **Local identities**: a per-seat product user id and display name (Couchlink already derives a unique id per
   party and seat for Steam; the EOS id would be derived the same way) in a small json beside the DLL.
3. **A local network layer**: discovery and message passing between the seats' DLLs (UDP broadcast or the same
   loopback port range as gbe's `custom_broadcasts`), used to implement lobbies, sessions, presence and P2P.
4. **Login and entitlement answers**: `EOS_Auth_Login` and `EOS_Connect_Login` succeed offline; `Ecom` reports
   ownership only of what the player has (same rule as DLC: never unlock what the player does not own).
5. **Test hooks**: a fake EOS client in `testgame` so the session runner's tests can check two copies find each other.

Licence options:

* **Use an existing emulator (Nemirtingas Epic Emulator)**: it is what the recipe authors used, but its licence has
  not been confirmed for this note (the only public GitHub copy found has no licence file and holds example
  configs only). Until a licence that allows redistribution is confirmed from the author's own source repository,
  Couchlink must not bundle it. If it is allowed, treat it exactly like gbe: a hash-pinned download the user's
  copy fetches, kept as separate unmodified files with the licence text beside them.
* **Ask the author** for permission to download and use it, and record the answer in the repo.
* **Write our own**, clean room, as a separate DLL published under LGPL-3 (same story as gbe_fork: nothing links
  against it, the DLL can be replaced). Epic's SDK headers and import library are under Epic's own SDK terms, so
  they cannot be copied into an LGPL repository; the ABI would be rebuilt from the public API documentation and the
  export table of the game's own EOS DLL. This is a large piece of work (hundreds of entry points) and should start
  with only what the five games above call, found by logging calls through a recording shim.

Decision for now: nothing EOS is built or downloaded, and the five recipes stay `needs-features`.
