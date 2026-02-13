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

## 次フェーズ: GPU スイッチの追加

### 方針

Settings UI に **「GPU を利用する」スイッチ** を追加する。macOS（Metal）と共通の UX。

- **デフォルト: OFF**（CPU 版で確実に動作）
- ユーザーが明示的に ON にすると Vulkan 経由で GPU を利用
- 動作しなければユーザー自身で OFF に戻せる

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

### 実装時の変更箇所

1. **Settings UI**: GPU 利用スイッチを追加
2. **`loader.ts`**: 設定値を参照し、OFF なら GPU 候補（metal, vulkan, cuda）をスキップ
3. **`intel-macos-whisper-worker.md` の修正2（ハードコードスキップ）を廃止**: スイッチに置き換え
4. **`release.yml`**: Vulkan ビルドステップを有効化

## 補足: ARM 版 Surface (Snapdragon X)

Surface Pro 11 / Surface Laptop 7 の一部モデルは Snapdragon X Elite/Plus (ARM) を搭載している。
現在のビルドは `win32-x64` のみで、`win32-arm64` 向けバリアントは生成していない。

- Windows の x64 エミュレーションで動作する可能性はあるが、パフォーマンスが落ちる
- ネイティブ対応するには `win32-arm64` 向けの whisper.node を別途ビルドする必要がある
- 現時点では未検証・未対応

**ARM 版 Surface のユーザーがいた場合は要対応。** 優先度はユーザーの有無に応じて判断する。
