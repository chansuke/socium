---
number: 1
title: Record architecture decisions
status: accepted
date: 2026-11-12
---

# Record architecture decisions

## Context and Problem Statement

Socium の設計判断はチケット（`docs/tickets/`）と分析文書に散らばっていて、何をいつ、どの選択肢と比べて決めたのかを後から辿りにくい。チケットは作業の単位で、判断の記録には向かない。

## Decision Drivers

* 判断の理由と、退けた選択肢を残す。
* 番号で参照でき、判断を改めるときに古い判断との関係（supersedes、amends）を示せる。
* 記録の形式を CI で検査できる。

## Considered Options

* `docs/adr/` に MADR 形式の ADR を置き、[adrs](https://github.com/joshrotenberg/adrs) で管理する。
* チケットや設計文書の中に判断を書く。

## Decision Outcome

Chosen option: 「`docs/adr/` に MADR 形式の ADR を置き、adrs で管理する」。Michael Nygard の [Documenting Architecture Decisions](https://www.cognitect.com/blog/2011/11/15/documenting-architecture-decisions) に従う。

* 設定は `adrs.toml`（NextGen mode の YAML frontmatter、MADR full template、新規作成時は `proposed`）。
* `adrs` は flake の `packages.adrs` と devShell に入れ、版を固定する。
* 本文は日本語で書き、見出しは MADR のまま英語にする。
* 詳しい仕様は設計文書（例: [design-agent-activity-stream.md](../design-agent-activity-stream.md)）に置き、ADR は判断と理由に絞る。

### Consequences

* Good, because 判断の履歴が番号付きで一か所に残る。
* Bad, because 判断を変えるたびに ADR の追加と目次の更新が要る。

### Confirmation

`.github/workflows/adr.yaml` が PR と main への push で `adrs doctor` を実行し、`adrs generate toc` の出力と `docs/adr/README.md` が一致することを確認する。
