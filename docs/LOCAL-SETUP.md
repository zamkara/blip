# Tutorial persiapan device lokal Blip

Jalankan dari root project:

```sh
cd /home/zam/Projects/blip
git fetch origin
git switch feature/blip-webhook-mvp
git pull --ff-only
git branch --show-current
git status --short
```

Branch harus `feature/blip-webhook-mvp` dan status idealnya kosong.

## Build dan konfigurasi

```sh
rustc --version
cargo --version
cargo fmt --check
cargo test
cargo build --release
cp blip.local.example.toml blip.toml
chmod 600 blip.toml
chmod +x scripts/test-deploy.sh
```

Edit `blip.toml` dan isi `signing_token` dengan token GitLab yang diawali `whsec_`. Token hanya disimpan lokal; `.gitignore` mengabaikan `blip.toml`.

```sh
./target/release/blip --config blip.toml validate
```

Output yang diharapkan: `configuration valid: 1 project(s)`.

## Uji script deploy lokal

```sh
./scripts/test-deploy.sh
test -s .blip-test-deploy.log
tail -n 5 .blip-test-deploy.log
```

Script ini hanya menulis log lokal.

## Jalankan server dan uji endpoint

Terminal pertama:

```sh
./target/release/blip --config blip.toml serve
```

Output harus menunjukkan `blip listening on 127.0.0.1:8080`.

Terminal kedua:

```sh
curl -i -X POST http://127.0.0.1:8080/webhook/blip-test \
  -H 'Content-Type: application/json' \
  -H 'X-Gitlab-Event: Push Hook' \
  -H 'X-Gitlab-Token: token-salah' \
  --data '{"ref":"refs/heads/dev"}'
```

Respons yang diharapkan adalah `401 Unauthorized`.

## Setelah lokal siap

Baru pasang webhook GitLab dengan URL `/webhook/blip-test`, isi **Signing token** `whsec_...`, kosongkan **Secret token**, dan centang **Push events**. Gunakan **Test → Push events**, lalu merge `feature/blip-webhook-mvp` ke `dev`.

Periksa hasil:

```sh
tail -n 20 .blip-test-deploy.log
./target/release/blip --config blip.toml history blip-test
```

