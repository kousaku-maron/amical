# Windows: Whisper GPU/CPU 構成の設計方針

## 背景

Windows 版リリースにあたり、whisper.node のバイナリ構成（GPU/CPU）をどうするか検討した。
Intel Mac で発生した GPU Timeout 問題（`intel-macos-whisper-worker.md` 参照）を踏まえ、Windows でも同様のリスクを回避する方針を決定した。

## ターゲットデバイス

主要ターゲットは Surface と ThinkPad。

| デバイス | 典型的な GPU | VRAM | 特記事項 |
|---|---|---|---|
| Surface Pro (10/11) | Intel Iris Xe / Intel Arc | 共有メモリ (~4-8GB) | Snapdragon X (ARM) モデルもあり |
| Surface Laptop (6/7) | Intel Iris Xe / Snapdragon X | 共有メモリ | ARM 版は win32-arm64 が必要 |
| Surface Go | Intel UHD 615/620 | 共有メモリ (~1.5GB) | Intel Mac と同レベルの低スペック GPU |
| ThinkPad X1 Carbon | Intel Iris Xe | 共有メモリ | dGPU なし |
| ThinkPad T/P シリーズ | Intel Iris Xe + NVIDIA MX/RTX | dGPU 搭載モデルあり | dGPU なら Vulkan/CUDA で高速化可 |

## Intel Mac の GPU Timeout 問題との類似性

Intel Mac では以下の問題が発生した:

1. Metal API で GPU バイナリの `dlopen` は**成功する**
2. しかし whisper.cpp の compute shaders 実行時に GPU がタイムアウトする
3. `loader.ts` のフォールバックは `ERR_DLOPEN_FAILED` のみ捕捉するため、ロード成功後のタイムアウトは検知できない

Windows + Vulkan でも同じパターンが起きうる:

- Intel 内蔵 GPU で Vulkan ドライバがインストール済み → `dlopen` 成功
- しかし VRAM 不足や性能不足で推論がタイムアウト/ハング
- CPU 版へのフォールバックが発動しない

## ビルド構成

### release.yml のビルドフロー

```yaml
# Step 1: CPU 版をビルド（全 Windows 環境で動作）
- name: Build whisper native binaries
  run: pnpm --filter @amical/whisper-wrapper build:native

# Step 2: Vulkan 版をビルド（GPU 搭載機で高速化）
- name: Build whisper native binaries (vulkan)
  if: matrix.os == 'windows'
  run: pnpm --filter @amical/whisper-wrapper build:native:vulkan
```

`build-native.js --vulkan` は `WHISPER_TARGETS = "win32-x64-vulkan,win32-x64"` を設定し、Vulkan + CPU の2バリアントをビルドする。

### パッケージに同梱されるバイナリ

| バイナリ | 用途 |
|---|---|
| `native/win32-x64-vulkan/whisper.node` | Vulkan GPU 版 |
| `native/win32-x64/whisper.node` | CPU 版（フォールバック） |

### ランタイムのロード順序 (`loader.ts`)

1. ~~`win32-x64-openblas`~~ — ビルドしないので存在しない
2. ~~`win32-x64-cuda`~~ — ビルドしないので存在しない
3. `win32-x64-vulkan` — 存在すれば `dlopen` 試行
4. `win32-x64` — Vulkan 失敗時のフォールバック
5. `cpu-fallback` — 最終手段

## 初回リリース方針

**CPU 版のみでリリースする。**

- `release.yml` の Vulkan ビルドステップ（Step 2）と Vulkan SDK インストールステップをコメントアウト
- 全 Windows 環境で安定動作を優先
- GPU 対応は次フェーズで追加

## 実装済み: GPU Acceleration スイッチ (PR #26)

Settings > Advanced に **「GPU Acceleration」スイッチ** を実装済み。macOS（Metal）と共通の UX。

- **Windows デフォルト: OFF**（CPU 版で確実に動作）
- ユーザーが明示的に ON にすると Vulkan 経由で GPU を利用
- 動作しなければユーザー自身で OFF に戻せる
- スイッチ変更後は再起動が必要（確認ダイアログあり）

### 自動検知/フォールバックを採用しない理由

検討した代替案と却下理由:

| 案 | 却下理由 |
|---|---|
| GPU デバイスタイプ自動検知 | whisper.node の C++ 側にコード追加が必要。内蔵/外付けの判定だけでは不十分（dGPU でも低スペックなら遅い） |
| VRAM 容量で判定 | Intel Arc 等の内蔵 GPU でも VRAM 表記が大きいケースがあり閾値設定が困難 |
| タイムアウトベースのフォールバック | タイムアウト時間（例: 10秒）待機後に CPU 版の起動・モデルロードが入り、合計 20秒以上の待ちが発生。体験が悪い |
| CPU + GPU 並列実行で速い方を採用 | モデル (~2GB) が 2 重ロードされメモリ 4GB 以上消費。8GB の Surface/ThinkPad では非現実的。内蔵 GPU の場合 VRAM がメインメモリ共有なので CPU 側も遅くなる逆効果あり |
| CPU ファースト + バックグラウンド GPU テスト | 実装可能だが、単純なスイッチで十分にカバーできる |

### スイッチ方式のメリット

- 実装がシンプル（`loader.ts` で GPU 候補をスキップするだけ）
- ユーザーに意図が明確に伝わる（「GPU を使うかどうか」の一択）
- macOS / Windows で共通の UX
- dGPU 搭載ユーザーは自覚があるので自分で ON にできる
- 問題発生時にユーザーが自力で OFF に戻せる

### 実装の仕組み

```
[Settings UI] GPU switch ON/OFF
  → tRPC mutation → DB に transcription.useGPU 保存
  → (再起動)
  → WhisperProvider → SimpleForkWrapper に WHISPER_USE_GPU="1"/"0" env を渡す
  → Worker fork 起動
    → loader.ts: candidateDirs() で WHISPER_USE_GPU をチェック → "0" なら GPU 候補スキップ
    → index.ts: binding.init({ use_gpu }) でランタイム GPU 制御
```

### 残タスク: Windows で GPU を有効化するには

1. **`release.yml`**: Vulkan ビルドステップのコメントアウトを解除
2. **Vulkan SDK**: CI に Vulkan SDK インストールステップを追加
3. **動作検証**: dGPU 搭載 Windows 機で GPU ON の動作確認

## 補足: ARM 版 Surface (Snapdragon X)

Surface Pro 11 / Surface Laptop 7 の一部モデルは Snapdragon X Elite/Plus (ARM) を搭載している。
現在のビルドは `win32-x64` のみで、`win32-arm64` 向けバリアントは生成していない。

- Windows の x64 エミュレーションで動作する可能性はあるが、パフォーマンスが落ちる
- ネイティブ対応するには `win32-arm64` 向けの whisper.node を別途ビルドする必要がある
- 現時点では未検証・未対応

**ARM 版 Surface のユーザーがいた場合は要対応。** 優先度はユーザーの有無に応じて判断する。

## 追記 (2026-02-15): Windows 起動時の `onnxruntime_binding.node` エラー

PR #27 のビルド成果物で、初回起動時に以下のエラーが報告された:

- `The specified module could not be found. ... onnxruntime_binding.node`
- `A dynamic link library (DLL) initialization routine failed. ... onnxruntime_binding.node`

### 原因

`onnxruntime_binding.node` の依存 DLL として `msvcp140.dll` / `vcruntime140.dll` / `vcruntime140_1.dll` を同梱していたが、  
実際には `onnxruntime.dll` の推移依存で `MSVCP140_1.dll` も必要だった。

そのため、VC++ 再頒布可能パッケージが入っていない Windows 環境では、起動時にネイティブモジュールのロードが失敗した。

### 対応

`apps/desktop/forge.config.ts` の `postPackage` フックで同梱する VC++ ランタイム DLL に
`msvcp140_1.dll` を追加した。

再発防止として、DLL の配置先をアプリ実行ファイル直下だけでなく
`onnxruntime_binding.node` と同じディレクトリ
(`resources/app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v6/win32/x64`) にも追加した。

### 検証時の注意 (Squirrel / 同一バージョン再インストール)

`Setup.exe` を再実行するだけでは、同一バージョン (`0.0.6`) の場合に
`%LocalAppData%\Grizzo\app-0.0.6` が更新されず、古いファイルが残ることがある。

同じエラーが継続する場合は、以下のクリーン手順で再検証する。

1. Grizzo をアンインストール
2. `%LocalAppData%\Grizzo` を削除
3. 最新 artifact の `Setup.exe` を実行

`nupkg` は更新用パッケージであり、手動実行は不要。

## 追記 (2026-02-16): 起動プロセスは存在するがウィンドウが表示されない問題

DLL 対応後、以下の状態が確認された:

- `app-0.0.6\resources\app.asar.unpacked\node_modules\onnxruntime-node\bin\napi-v6\win32\x64` に
  `onnxruntime_binding.node` / `onnxruntime.dll` / VC++ ランタイム DLL が存在
- `Grizzo.exe` 実行後に `Grizzo.exe` プロセスが複数起動している
- しかしメインウィンドウが表示されない

このため、原因は `onnxruntime` の DLL 不足ではなく、起動シーケンス中に
`ServiceManager.initialize()` が完了せず、ウィンドウ作成まで進めないケースと判断した。

### 対応方針

`apps/desktop/src/main/managers/service-manager.ts` の
`TranscriptionService.initialize()` 呼び出しにタイムアウト (15 秒) を導入し、
初期化がハングしても UI 起動をブロックしないようにする。

- 成功時: 従来どおり transcription を有効化
- 失敗/タイムアウト時: transcription を `null` として継続起動
- ログに timeout/error を残して後続調査を可能にする

## 追記 (2026-02-16): VAD 初期化ハングの根本原因と恒久対応

Windows 10 `10.0.17134` 環境で、以下のログで起動が停止する事象を確認した。

- `Loading VAD model from ...silero_vad_v6.onnx`
- 以降 `VAD service initialized` が出ず、ウィンドウ作成まで進まない

### 根本原因

`onnxruntime-node` が lockfile 上 `1.22.0` に解決されており、実行環境の Windows ビルド (`17134`) と互換性がなかった。

- `apps/desktop/package.json` は `^1.20.1` 指定
- しかし lockfile は `1.22.0` を選択
- ONNX Runtime 1.22 系の Windows 要件 (10.0.19041+) を満たさない環境で
  `InferenceSession.create()` が正常完了せず初期化ハング

### 恒久対応

1. `apps/desktop/package.json` の `onnxruntime-node` を `1.20.1` に**厳密固定**  
   (`^1.20.1` → `1.20.1`)
2. `pnpm-lock.yaml` を更新し、`onnxruntime-node` / `onnxruntime-common` を `1.20.1` に固定

これにより、古い Windows 10 ビルドを含む環境でも VAD 初期化の互換性リスクを下げる。

### 補足

`ServiceManager` 側の VAD 初期化タイムアウト/フォールバックは、将来の環境差異に対する
起動不能回避のセーフティネットとして維持する。
