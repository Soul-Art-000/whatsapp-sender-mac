# Katkı Rehberi

## Dallar
- Varsayılan dal `main`.
- Çalışma dalları: `feat/<konu>`, `fix/<konu>`, `chore/<konu>`.

## Commit mesajları
Conventional Commits:
`feat:`, `fix:`, `docs:`, `refactor:`, `perf:`, `test:`, `build:`, `ci:`, `chore:`
Başlık ≤ 72 karakter, gövde opsiyonel. Türkçe yazılabilir.

## Pull request
1. `main`'i güncelle, dalını rebase et.
2. Yerelde çalıştır/derle (bkz. README).
3. PR şablonundaki kontrol listesini doldur.
4. Sır, `.env`, model/medya gibi büyük dosyaları commit etme (bkz. `.gitignore`).

## Sürüm
SemVer: etiket `v<major>.<minor>.<patch>`; değişiklikleri `CHANGELOG.md`'ye yaz.
