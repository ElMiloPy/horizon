<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/media/readme/title-dark.png">
    <img src="docs/media/readme/title-light.png" alt="Horizon" width="320">
  </picture>
</p>

<p align="center">
  <b>Minecraft 26.3, rewritten from scratch in Rust.</b><br>
  Play on existing worlds and servers.
</p>

> [!NOTE]
> **Personal Fork Notice & Attribution:** This is a personal Android fork/repository of Horizon and is **not meant to be perfect**.
> The Android touch control system and button icon assets, the in-game Microsoft & offline account switcher, the in-game skin picker, and the Android performance optimizations (interior leaf canopy culling, mobile terrain upload budgeting, worker-thread cap, and bounded mesh/audio cache budgets) are copied and adapted from [starling-client/starling](https://github.com/starling-client/starling).

### Fork Features

> Keep this list updated whenever a new fork-specific feature is added.

- **Android Port (`arm64-v8a`):** Native Android `NativeActivity` port built via `packaging/android/build_apk.py` (works on-device in Termux or with Android SDK), using Android's native system allocator (`system-allocator`) and 16 KB page alignment.
- **Ultra-Compact Bundled Assets (`resources.hpk`):** Solid LZMA-compressed resource bundle containing game textures, models, optimized gameplay sounds, and verified Starling/Bedrock touch icon sprites (~24.5 MB total).
- **Touch Controls & Icons (from Starling):** All 4 Starling control schemes (Joystick + Crosshair, Joystick + Tap, D-Pad + Tap, D-Pad + Crosshair), authentic Bedrock/Starling touch icon sprites (with procedural MCPE bevel fallback), in-game Touch Settings panel, Esc button on inventory/container screens, hotbar taps, and tap-to-attack mobs.
- **In-Game Offline & Online (Microsoft) Accounts (from Starling):** Switch, add, and remove offline accounts or sign in with Microsoft (OAuth 2.0 Device Code -> Xbox Live -> XSTS -> Minecraft Services) directly from the Title Screen without restarting.
- **In-Game Skin Picker (from Starling):** Browse local PNG skins, import 64x64 / 64x32 skins from Downloads, preview Classic/Slim models in 2D, apply live in singleplayer and multiplayer, and upload to Microsoft profiles.
- **Android Performance Optimizations (from Starling):** Interior leaf canopy culling (`LeafCullingMode::Solid`), 2 MiB per-frame mobile terrain upload cap, Android worker-thread cap (`<= 4`), and reduced mesh pool (8 MiB) & decoded audio cache (24 MiB) budgets.
- **Zero Bloatware & Upstream-Friendly Simplifications:** Lean, self-contained modules with minimal diffs in upstream files so rebasing onto `horizon-rust/horizon` `main` stays simple and low-friction.

<p align="center">
  <a href="https://playhorizon.dev/download"><img src="docs/media/readme/download-button.png" alt="Download the Horizon launcher for Windows, macOS and Linux" width="400"></a><br>
  <sub>You need your own copy of Minecraft: Java Edition.</sub>
</p>

<p align="center">
  <a href="https://playhorizon.dev/docs/install"><b>Install guide</b></a> ·
  <a href="https://playhorizon.dev/docs/"><b>Docs</b></a> ·
  <a href="https://playhorizon.dev/mods"><b>Mods</b></a> ·
  <a href="https://discord.gg/playhorizon"><b>Discord</b></a>
</p>

<p align="center">
  <img src="docs/media/readme/hero-lookout.webp" alt="A player's view from a wooden lookout over a village: a friend in diamond armour sneaks at the edge while a creeper flashes white behind her, another friend rides a horse between the houses below, and forest, snowy peaks and the sea run to the horizon" width="100%">
</p>

<p align="center">
  <img src="docs/media/readme/signs.webp" alt="Three wooden signs: 3.5x faster start than modded Java, 38 percent more FPS than Java with Sodium, and a mod API to port your mods to Rust" width="100%">
</p>

<p align="center">
  <sub>*Against Java Edition with Sodium, Lithium, FerriteCore and ImmediatelyFast on a Ryzen 7 9800X3D with an RTX 5080. More computers on the <a href="https://playhorizon.dev/benchmarks">benchmarks page</a>.</sub>
</p>

<h2 align="center">What you get</h2>

- **Vanilla Minecraft:** survival, creative and hardcore in every dimension, with the same terrain, mobs, villagers and redstone. Your saves open as they are.
- **Multiplayer:** vanilla and Horizon servers, LAN, and invite codes for playing your world with friends. You can [host a server](https://playhorizon.dev/docs/hosting) too.
- **Mods:** Horizon's own mod system, with mods like zoom and a minimap, and shader packs. Browse the [catalogue](https://playhorizon.dev/mods) or [make a mod](https://playhorizon.dev/docs/make-a-mod).
- **Beyond vanilla:** terrain up to 1,024 chunks away, HDR, controller support, graphics presets and dynamic lights.
- **Dedicated server:** Server software for high performance, low memory usage, and near instant startup. See [the server benchmark](docs/performance/server-vs-vanilla.md).

<h2 align="center">What you need</h2>

- **Your own copy of Minecraft: Java Edition.** Horizon ships no Minecraft files. It builds them from the game you bought, so install version 26.3 and start it once.
- **Windows, macOS or Linux:** Windows on x64 or ARM64, macOS 12 or later, Linux on x86-64.

<h2 align="center">Made by the Horizon Team</h2>

<table align="center">
  <tr>
    <td align="center" width="140"><a href="https://github.com/kacpherek"><img src="https://github.com/kacpherek.png?size=160" width="80" alt="Kacper"><br><b>Kacper</b></a></td>
    <td align="center" width="140"><a href="https://github.com/Online5277"><img src="https://github.com/Online5277.png?size=160" width="80" alt="Cosmic"><br><b>Cosmic</b></a></td>
    <td align="center" width="140"><a href="https://github.com/levzzz5154"><img src="https://github.com/levzzz5154.png?size=160" width="80" alt="levzzz"><br><b>levzzz</b></a></td>
    <td align="center" width="140"><a href="https://github.com/rdbtCVS"><img src="https://github.com/rdbtCVS.png?size=160" width="80" alt="rdbt"><br><b>rdbt</b></a></td>
    <td align="center" width="140"><a href="https://github.com/dezer-x"><img src="https://github.com/dezer-x.png?size=160" width="80" alt="Anthony S"><br><b>Anthony S</b></a></td>
    <td align="center" width="140"><a href="https://github.com/NikitaMGrimm"><img src="https://github.com/NikitaMGrimm.png?size=160" width="80" alt="Nikita"><br><b>Nikita</b></a></td>
    <td align="center" width="140"><a href="https://github.com/ludovic111"><img src="https://github.com/ludovic111.png?size=160" width="80" alt="Ludovic Marie"><br><b>Ludovic Marie</b></a><br><sub>Founder</sub></td>
  </tr>
</table>

<p align="center">
  <sub>and everyone who has contributed</sub><br><br>
  <a href="https://github.com/horizon-rust/horizon/graphs/contributors"><img src="https://contrib.rocks/image?repo=horizon-rust/horizon&max=72&columns=12" alt="Everyone who has contributed to Horizon"></a>
</p>

<p align="center">
  <a href="https://blacksmith.sh"><img src="docs/media/readme/blacksmith.png" alt="CI powered by Blacksmith" width="280"></a>
</p>

<p align="center">
  <a href="docs/native-client.md#run-from-a-checkout"><b>Build guide</b></a> ·
  <a href="CONTRIBUTING.md"><b>Contributing</b></a> ·
  <a href="GOVERNANCE.md"><b>Governance</b></a> ·
  <a href="https://github.com/sponsors/ludovic111"><b>Sponsor</b></a>
</p>

<sub>Horizon is released under the [GNU LGPL, version 3 or later](LICENSE), for the project's original work. Code merged before the move from MIT stays available under the [MIT licence](LICENSES/MIT.txt). Dependencies keep their own licences, see the [notices](THIRD_PARTY_NOTICES.md). Founded by [Ludovic Marie](https://lu4ovic.com) and built by the Horizon Team and [its contributors](https://github.com/horizon-rust/horizon/graphs/contributors). You can support it on [GitHub Sponsors](https://github.com/sponsors/ludovic111).</sub>

<sub>NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR MICROSOFT. Minecraft is a trademark of Mojang Synergies AB; its code and assets belong to Mojang and Microsoft under the [Minecraft EULA](https://www.minecraft.net/en-us/eula).</sub>
