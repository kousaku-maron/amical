# Intel Mac: Whisper ワーカー問題調査報告

## 問題概要

Intel版MacBookのリリースビルドで、マイクはオンになるが発話時に「Speech Not Detected」と表示され、音声文字起こしが一切動作しない。

## 環境情報

- **プラットフォーム**: darwin, arch: x64 (Intel Mac, i5-8210Y)
- **GPU**: Intel UHD Graphics 617 (VRAM: 1536MB)
- **アプリバージョン**: 0.0.2
- **モデル**: whisper-large-v3-turbo

## 根本原因（2つの独立した問題）

### 原因1: Node.js バイナリの JIT エンタイトルメント欠落 → SIGTRAP

**Node.js バイナリにコード署名エンタイトルメント (`com.apple.security.cs.allow-jit`) が適用されていないため、V8 JIT コンパイラが executable memory を確保できず SIGTRAP でクラッシュする。**

#### 直接原因: forge.config.ts の optionsForFile パスマッチングバグ

```typescript
// forge.config.ts (修正前)
optionsForFile: (filePath: string) => {
  if (filePath.includes("node-binaries")) {
    // ← パッケージ後のパスに "node-binaries" は含まれない！
    return { entitlements: "./entitlements.node.plist", hardenedRuntime: true };
  }
  return null; // ← Node バイナリもここに落ちる → エンタイトルメントなし
}
```

**問題**: `extraResource` が `./node-binaries/darwin-x64/node` を `Grizzo.app/Contents/Resources/node` にコピーする。
`optionsForFile` に渡されるパスは**パッケージ後のパス** (`/path/to/Grizzo.app/Contents/Resources/node`) であり、ソースパスの `"node-binaries"` を含まない。

#### なぜ Apple Silicon (arm64) では動くのか

V8 の JIT 実装がアーキテクチャにより異なる:
- **arm64**: `MAP_JIT` + `pthread_jit_write_protect_np()` を使用。スレッドレベルの W^X 切替であり、`mprotect` によるページ権限変更を行わない
- **x86_64**: 従来の `mprotect()` で実行権限を付与。Hardened Runtime 下では `com.apple.security.cs.allow-jit` が必須

#### ログ証拠

```
[14:29:25.922] [worker stderr] #
# Fatal error in , line 0
# Check failed: 12 == (*__error()).
```

スタックトレース:
```
v8::base::OS::SetPermissions()                    ← mprotect が errno=12 (ENOMEM) で失敗
→ MemoryAllocator::SetPermissionsOnExecutableMemoryChunk()
  → AllocateAlignedMemory() → AllocatePage()
    → Factory::CodeBuilder::BuildInternal()
      → BaselineCompiler::Build()                  ← V8 JIT ベースラインコンパイラ
        → GenerateBaselineCode()
          → Compiler::CompileBaseline()
            → TieringManager::OnInterruptTick()    ← JS実行中のJIT最適化トリガー
```

#### 障害発生フロー

```
Worker fork 起動 (execPath: Resources/node)
  → Node.js (V8) がワーカースクリプトを解釈開始
  → JS コード実行中に V8 TieringManager が JIT コンパイルをトリガー
  → BaselineCompiler::Build() → CodeBuilder::BuildInternal()
  → MemoryAllocator::AllocatePage(Executability::kExecutable)
  → mprotect(addr, size, PROT_READ | PROT_EXEC)
  → macOS Hardened Runtime: allow-jit エンタイトルメントなし → ENOMEM
  → V8_Fatal("Check failed: 12 == (*__error())")
  → SIGTRAP
  → Worker プロセス異常終了
  → Whisper preload 全滅
  → "No speech detected" 表示
```

### 原因2: Metal GPU Timeout → SIGABRT

JIT 問題を修正した後、Whisper ワーカーは起動するようになったが文字起こしは依然として動作しなかった。
`darwin-x64-metal/whisper.node` が自動的にロードされ、Intel UHD Graphics 617 で Metal compute shaders が実行されるが、GPU のスペック不足によりタイムアウトする。

#### ログ証拠

```
ggml_metal_synchronize: error: command buffer 1 failed with status 5 = Error
Caused GPU Timeout Error (kIOAccelCommandBufferCallbackErrorTimeout)
```

#### なぜ Metal が Intel GPU で動かないのか

1. **メモリ不足**: Intel UHD 617 の `recommendedMaxWorkingSetSize` は ~1611MB。whisper-large-v3-turbo モデルは ~2000MB 必要
2. **Compute Shader 非互換**: whisper.cpp の Metal compute shaders は Apple GPU (Apple Silicon) 向けに最適化されており、Intel GPU ではシェーダー実行がタイムアウトする
3. **OS レベルのタイムアウト**: macOS の GPU コマンドバッファタイムアウトはアプリケーションから延長不可

## 修正内容

### 修正1: `apps/desktop/forge.config.ts` — エンタイトルメントのパスマッチ修正

```typescript
// 修正後
optionsForFile: (filePath: string) => {
  if (
    filePath.includes("node-binaries") ||
    filePath.endsWith("/Resources/node")
  ) {
    return { entitlements: "./entitlements.node.plist", hardenedRuntime: true };
  }
  return null as any;
}
```

> **補足**: 最初 `filePath.endsWith("/node")` としたが、Mach-O 以外のファイル（`electron.asar/browser/api/node` 等）にもマッチしてしまい `codesign` が失敗したため、`"/Resources/node"` に限定。

### 修正2: `packages/whisper-wrapper/src/loader.ts` — GPU スイッチによる制御（ハードコードスキップ廃止）

~~当初は `candidateDirs()` で `metal` を `darwin-arm64` のみにハードコードスキップしていた。~~

PR #26 で **Settings UI の GPU Acceleration スイッチ** に置き換え済み。
環境変数 `WHISPER_USE_GPU` (`"0"` = GPU スキップ) で `candidateDirs()` が GPU 候補の有無を制御する。

```typescript
function candidateDirs(platform: string, arch: string): string[] {
  const useGPU = process.env.WHISPER_USE_GPU !== "0";
  const candidates = GPU_FIRST_CANDIDATES.filter((tag) => {
    if (!useGPU && GPU_TAGS.includes(tag)) return false;
    return true;
  });
  // ...
}
```

デフォルト値:
- **Apple Silicon Mac**: ON (Metal で高速動作の実績あり)
- **Intel Mac**: OFF (GPU Timeout の実績あり)
- **Windows**: OFF (未検証)

### 修正3: `packages/whisper-wrapper/bin/build-addon.js` — ビルド時の Metal 除外

#### 3a. `computeVariants()` — Metal バリアントの除外
`darwin-x64` では Metal バリアントをビルドしない。CI の無駄なビルド時間を削減。

#### 3b. `variantFromName()` — 非Metal バリアントで `GGML_METAL=OFF` を明示
whisper.cpp の CMake は macOS 上で `GGML_METAL` をデフォルトで有効化するため、
`darwin-x64` (非Metal) バリアントに `GGML_METAL=OFF` を明示設定。

## 調査の時系列

| フェーズ | 仮説 | 結果 |
|---|---|---|
| 初期調査 | Metal GPU 初期化クラッシュ | ❌ Metal 無効化後も SIGTRAP 再現 |
| CMake 調査 | GGML_METAL デフォルト ON | ✅ 修正したが根本原因ではなかった |
| 診断コード追加 | stderr キャプチャで特定 | ✅ V8 JIT クラッシュを発見 |
| 根本原因1 特定 | Node バイナリのエンタイトルメント欠落 | ✅ forge.config.ts のパスマッチングバグ |
| エンタイトルメント修正後 | 文字起こしが動くはず | ❌ Metal GPU Timeout で再度失敗 |
| 根本原因2 特定 | Intel GPU で Metal Timeout | ✅ loader.ts で Metal スキップにより解決 |

## 教訓

1. **`optionsForFile` のパスはパッケージ後のパス**: `extraResource` でコピーされたファイルのパスは、ソースディレクトリのパスとは異なる。パスマッチには注意が必要。
2. **SIGTRAP の原因は多様**: Metal だけでなく V8 JIT の `mprotect` 失敗でも SIGTRAP が発生する。stderr のキャプチャが調査の鍵。
3. **Metal ≠ Apple GPU 全般**: Metal API 自体は Intel GPU でも動作するが、whisper.cpp の compute shaders は Apple Silicon 専用に最適化されている。`dlopen` 成功後の GPU 初期化タイムアウトは `require()` のフォールバック機構では捕捉できない。
4. **Hardened Runtime の W^X は x86_64 特有の問題**: arm64 では `MAP_JIT` API を使うため影響しない。x86_64 のみ `com.apple.security.cs.allow-jit` が必須。

## 実装済み: Settings GPU Acceleration スイッチ (PR #26)

### 概要

ハードコードスキップを廃止し、Settings > Advanced に **「GPU Acceleration」スイッチ** を実装した。

### 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `apps/desktop/src/db/schema.ts` | `transcription.useGPU?: boolean` 追加 |
| `apps/desktop/src/trpc/routers/settings.ts` | `useGPU` の zod input 追加、`relaunchApp` mutation 追加 |
| `apps/desktop/src/renderer/.../AdvancedSettingsContent.tsx` | GPU スイッチ UI、再起動確認ダイアログ |
| `packages/whisper-wrapper/src/loader.ts` | `WHISPER_USE_GPU` 環境変数で GPU 候補制御 |
| `packages/whisper-wrapper/src/index.ts` | `binding.init()` に `use_gpu` パラメータ渡し |
| `apps/desktop/src/pipeline/.../whisper-worker-fork.ts` | `WHISPER_USE_GPU` env 読み取り → Whisper に渡す |
| `apps/desktop/src/pipeline/.../simple-fork-wrapper.ts` | `additionalEnv` でワーカーに環境変数伝搬 |
| `apps/desktop/src/pipeline/.../whisper-provider.ts` | `useGPU` フラグを SimpleForkWrapper に渡す |
| `apps/desktop/src/services/transcription-service.ts` | `resolveUseGPU()` でプラットフォームデフォルト決定、Mutex で排他制御 |
| `apps/desktop/src/main/preload.ts` | `isAppleSilicon` をレンダラーに公開 |

### 実装中に発見した追加バグ

GPU スイッチ実装後、スイッチ OFF でも whisper が GPU を使い続ける問題が判明。根本原因は2つ:

1. **`Whisper` クラスが `opts` パラメータを無視していた**: `_opts` として受け取り `binding.init()` に `use_gpu` を渡していなかった
2. **`whisper-worker-fork.ts` が `{ gpu: true }` をハードコードしていた**: `WHISPER_USE_GPU` 環境変数を読んでいなかった

`darwin-arm64` バイナリ自体にも Metal が組み込まれているため（CMake のデフォルト）、バイナリ選択だけでは GPU 制御できず、**ランタイムの `use_gpu` パラメータが実際の制御手段**となっている。

### 残課題

- **ビルド側の改善**: `darwin-arm64` (非Metal バリアント) に Metal が混入しないよう CMake で `GGML_METAL=OFF` を明示する (Low Priority)
- **`app.relaunch()` の開発モード制限**: 開発モードでは再起動時にレンダラーが黒画面になる。本番では正常動作。コメントで注記済み
