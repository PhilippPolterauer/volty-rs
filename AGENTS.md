# AGENTS.md

> **TL;DR:** Do the task, don't break stuff you shouldn't touch, and zero backward compatibility baggage. Move fast, ship clean.

---

## 🛑 Rule #0: IDGAF About Excuses

* **Get it done:** If something fails, retry, adapt, or fix it yourself before complaining.
* **No unnecessary fluff:** Skip "I hope this helps!", "As an AI language model...", or polite filler. Just deliver the output.
* **Zero fluff in communication:** Direct answers first. Explanations at the bottom only if required.

---

## ⚡ Backward Compatibility: IDGAF

* **No Legacy Baggage:** Do **NOT** preserve deprecated APIs, legacy wrappers, or backwards-compatible hacks unless explicitly requested.
* **Break It If It Makes It Better:** If modernizing a structure, signature, or pattern requires breaking old call sites or schema compatibility, **do it**. Clean architecture > historical compatibility.
* **Delete Dead Code:** Strip out fallback branches, legacy feature flags, and workaround adapters without asking.

---

## 🛠 Code & Output Standards

1. **Working Code Only:** Never output placeholders like `// TODO: implement this` or `/* rest of code here */`. Write the full, concrete implementation.
2. **Deterministic & Modern:** Write minimal, modern, idiomatic code. Zero bloat. Zero unneeded dependencies.
3. **Fail Fast:** If an action fails fatally or input is missing critical data, report the exact error clearly and stop—don't guess or make up mock data.

---

## 📁 File & Project Hygiene

* **Atomic Changes:** Edit what was requested.
* **No Validation Seeking:** Don't ask "Would you like me to update X?" if X is the obvious requirement to keep things working after a breaking change. Just do it.
* **Release Changelog:** When preparing a release, update `CHANGELOG.md` with the version, release date, and user-facing changes; move the released items out of `[Unreleased]`.

## 
if Python is needed use `uv`