# Third-party notices

The source repository and Windows distribution use third-party components. Their licenses remain applicable to those components.

| Component | License | Source |
| --- | --- | --- |
| Tauri | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| tauri-plugin-single-instance | Apache-2.0 OR MIT | https://github.com/tauri-apps/plugins-workspace |
| @tauri-apps/api | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| React | MIT | https://github.com/facebook/react |
| lucide-react 1.41.0 | ISC (inherited Feather icons retain MIT notices) | https://github.com/lucide-icons/lucide |
| livekit-client | Apache-2.0 | https://github.com/livekit/client-sdk-js |
| livekit-api | Apache-2.0 | https://github.com/livekit/rust-sdks |
| wouter | Unlicense | https://github.com/molefrog/wouter |
| zod | MIT | https://github.com/colinhacks/zod |
| rusqlite | MIT | https://github.com/rusqlite/rusqlite |
| sqlite-vec | MIT/Apache-2.0 | https://github.com/asg017/sqlite-vec |
| keyring | MIT OR Apache-2.0 | https://github.com/open-source-cooperative/keyring-rs |
| chrono | MIT OR Apache-2.0 | https://github.com/chronotope/chrono |
| serde | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| reqwest | MIT OR Apache-2.0 | https://github.com/seanmonstar/reqwest |
| tungstenite | MIT OR Apache-2.0 | https://github.com/snapview/tungstenite-rs |
| uuid | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid |
| thiserror | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| tracing | MIT | https://github.com/tokio-rs/tracing |
| tracing-subscriber | MIT | https://github.com/tokio-rs/tracing |
| ts-rs | MIT | https://github.com/Aleph-Alpha/ts-rs |
| zeroize | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| sha2 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| base64 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| pdf-extract | MIT | https://github.com/jrmuizel/pdf-extract |
| docx-rs | MIT | https://github.com/bokuweb/docx-rs |
| windows-sys | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| NAudio.Wasapi | MIT | https://github.com/naudio/NAudio |
| silero-vad-rust | MIT (ships the bundled Silero VAD ONNX model) | https://github.com/sheldonix/silero-vad-rust |
| Silero VAD ONNX model (bundled by silero-vad-rust) | MIT | https://github.com/snakers4/silero-vad |
| ort | MIT OR Apache-2.0 (statically links ONNX Runtime) | https://github.com/pykeio/ort |
| ONNX Runtime | MIT | https://github.com/microsoft/onnxruntime |
| rustfft | MIT OR Apache-2.0 | https://github.com/ejmahler/RustFFT |
| obws | MIT | https://forge.dnaka91.rocks/dnaka91/obws |
| tauri-plugin-global-shortcut | Apache-2.0 OR MIT | https://github.com/tauri-apps/plugins-workspace |
| smart-turn v3 model, smart-turn-v3.2-cpu.onnx (bundled at resources/models/) | BSD-2-Clause | https://github.com/pipecat-ai/smart-turn |

Licenses above are taken from the locked crate / npm / NuGet metadata for the versions this repository depends on; bundled model asset licenses are taken from the upstream repository LICENSE files. Transitive crates keep their own licenses.

Portable OBS and VB-CABLE are not packaged or started by the current Tauri build. Path and probe code can use a local copy under `resources/prerequisites` if you place one there; that leftover is not a managed OBS / virtual-cam product path.

## lucide-react 1.41.0 license

ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

---

The following Lucide icons are derived from the Feather project:

airplay, alert-circle, alert-octagon, alert-triangle, aperture, arrow-down-circle, arrow-down-left, arrow-down-right, arrow-down, arrow-left-circle, arrow-left, arrow-right-circle, arrow-right, arrow-up-circle, arrow-up-left, arrow-up-right, arrow-up, at-sign, calendar, cast, check, chevron-down, chevron-left, chevron-right, chevron-up, chevrons-down, chevrons-left, chevrons-right, chevrons-up, circle, clipboard, clock, code, columns, command, compass, corner-down-left, corner-down-right, corner-left-down, corner-left-up, corner-right-down, corner-right-up, corner-up-left, corner-up-right, crosshair, database, divide-circle, divide-square, dollar-sign, download, external-link, feather, frown, hash, headphones, help-circle, info, italic, key, layout, life-buoy, link-2, link, loader, lock, log-in, log-out, maximize, meh, minimize, minimize-2, minus-circle, minus-square, minus, monitor, moon, more-horizontal, more-vertical, move, music, navigation-2, navigation, octagon, pause-circle, percent, plus-circle, plus-square, plus, power, radio, rss, search, server, share, shopping-bag, sidebar, smartphone, smile, square, table-2, tablet, target, terminal, trash-2, trash, triangle, tv, type, upload, x-circle, x-octagon, x-square, x, zoom-in, zoom-out

The MIT License (MIT) (for the icons listed above)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## smart-turn v3 model license (BSD-2-Clause)

The bundled smart-turn-v3.2-cpu.onnx model is from the pipecat-ai/smart-turn
project and is distributed under the following license:

BSD 2-Clause License

Copyright (c) 2024–2025, Daily

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
