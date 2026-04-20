# evelin

`evelin` je Rust knihovna a CLI pro evaluations nad assety modernich AI agentu, hlavne pro skilly a navazujici test fixtures. Tohle repo je novy zdroj pravdy pro dalsi vyvoj i distribuci; puvodni Python experiment z `/Users/jiri/agents/tests/src/common` je sem prevedeny jako prvni funkcni Rust baseline.

## Co umi

- nacist eval konfigurace z YAML, JSON a TOML
- validovat `suite.eval.yaml` proti bundled schema pravidlum
- lintovat gate requirements nad `SKILL.md`
- spoustet live evaly pres `codex exec`
- aplikovat globalni runtime overlay z `eval-config.toml`
- orchestrit per-skill suite flow nad vice eval configy
- vystavovat vsechno jako reusable Rust library i jako CLI

## Architektura

- `core/src/config.rs`: konfigurace, project layout, runtime overlay
- `core/src/schema.rs`: schema validator a bundled schema kontrakt
- `core/src/gate.rs`: gate lint loading a reporting
- `core/src/runtime.rs`: `codex exec`, timeouty, retry a `CODEX_HOME` isolation
- `core/src/eval.rs`: case execution a marker grading
- `core/src/suite.rs`: schema/gate/eval orchestrator pro skill suite
- `core/src/main.rs`: CLI entrypoint
- `core/src/skill-suite.schema.yaml`: bundled schema dokument pro suite configy
- `core/src/eval-config.toml`: bundled default runtime config template

## CLI

```bash
cargo run -- schema-lint --config path/to/suite.eval.yaml --out out/schema.json
cargo run -- gate-lint --requirements path/to/suite.eval.yaml --out out/gate.json
cargo run -- eval --config path/to/suite.eval.yaml --out out/eval.json
cargo run -- suite --skill scope-to-acceptance
```

## Predpokladany layout testovaneho projektu

Strategie je zamerne podobna Gradlu: minimum v rootu, zbytek v dedikovanem adresari.

- root projektu:
  - nativni eval config soubor, typicky `*.eval.yaml` nebo `eval.yaml`
- `eval-config.toml` pro runtime defaults v rootu testovaneho repozitare
- `.evelin/`:
  - extra assety, fixture, generated reports nebo pozdeji projektove extension body
- `skills/`:
  - testovane skill dokumenty
- `tests/src/skills/`:
  - per-skill suite configy a legacy fixtures pro orchestrated suites

Aktualni implementace uz pouziva project root, `skills/`, `tests/src/skills/` a `eval-config.toml` v testovanem projektu. `.evelin/` je rezervovana v `ProjectLayout` pro dalsi rozsireni bez rozbiti API.

## Runtime konfigurace

Bundled default template je v [core/src/eval-config.toml](/Users/jiri/projects/evelin/core/src/eval-config.toml). Pri realnem behu se ale efektivni runtime sklada z `eval-config.toml` v rootu testovaneho projektu takto:

1. vestavene defaults
2. `eval-config.toml` `[defaults]`
3. `eval-config.toml` `[eval_type.<type>]`
4. inline hodnoty v konkretnim eval configu
5. `EVAL_CODEX_HOME_BASE_DIR`

## Instalace z release artifactu

Downstream agent asset repozitare muzou stahnout konkretni verzi pomoci [scripts/install-evelin.sh](./scripts/install-evelin.sh).

```bash
./scripts/install-evelin.sh 0.1.0
```

Skript:

- detekuje podporovany OS/arch a vybere odpovidajici artifact pro publikovane targety (`linux/x86_64`, `macOS/aarch64`)
- stahne `SHA256SUMS` i konkretni archive
- overi checksum pred instalaci
- nainstaluje binarku do `~/.local/bin` nebo do `EVELIN_INSTALL_DIR`
- na Windows zatim ocekava manualni stazeni publikovaneho `.zip` artifactu

Konfigurace zdroje artifactu:

- `EVELIN_RELEASE_BASE_URL`:
  - explicitni base URL, napriklad `https://s3.eu-west-1.amazonaws.com/my-bucket/evelin`
- nebo odvozeni ze S3:
  - `EVELIN_S3_BUCKET` povinny
  - `EVELIN_S3_REGION` optional, default `eu-west-1`
  - `EVELIN_S3_PREFIX` optional, default `evelin`

Priklad pro downstream repo:

```bash
EVELIN_S3_BUCKET=my-release-bucket \
EVELIN_S3_PREFIX=evelin \
./scripts/install-evelin.sh v0.1.0
```

## Vyvoj

```bash
cargo fmt
cargo test
```

Knihovna je navrzena tak, aby slo stejnou logiku pouzit jak z CLI, tak z dalsich Rust entrypointu nebo budoucich distributovanych wrapperu.
