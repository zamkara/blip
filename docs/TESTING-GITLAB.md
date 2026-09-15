# Panduan uji Blip dengan GitLab

Skenario uji menggunakan branch kerja `feature/blip-webhook-mvp`, lalu merge ke `dev`. Karena merge menghasilkan push baru ke `dev`, konfigurasi Blip memfilter event `push` dan branch `dev`.

## 1. Siapkan konfigurasi lokal

Jalankan di root project Blip:

```sh
cp blip.local.example.toml blip.toml
chmod +x scripts/test-deploy.sh
```

Edit `blip.toml`, isi `signing_token` dengan token GitLab yang diawali `whsec_`. Token ini hanya ditampilkan sekali. `secret` bersifat opsional sebagai fallback kompatibilitas.

Validasi dan build:

```sh
cargo run -- --config blip.toml validate
cargo build --release
```

## 2. Buat repository GitLab dan push branch kerja

Buat project kosong di GitLab, lalu jalankan command berikut dengan URL repository Anda:

```sh
git init
git add .
git commit -m "feat: initial Blip webhook MVP"
git branch -M feature/blip-webhook-mvp
git remote add origin https://gitlab.com/USERNAME/REPOSITORY.git
git push -u origin feature/blip-webhook-mvp
```

Di GitLab, buat branch `dev` dari branch tersebut atau buat merge request menuju `dev`.

## 3. Jalankan server Blip

Di device yang sama, jalankan:

```sh
cargo run -- --config blip.toml serve
```

Server mendengarkan `127.0.0.1:8080`. Jika GitLab tidak berada di device yang sama, ubah `bind` dan gunakan tunnel/reverse proxy yang Anda kelola sendiri.

## 4. Pasang webhook GitLab

Pada GitLab: **Settings → Webhooks**.

Referensi resmi: [GitLab Webhooks](https://docs.gitlab.com/user/project/integrations/webhooks/).

- URL: `http://HOST-ATAU-TUNNEL:8080/webhook/blip-test`
- Signing token: isi token `whsec_...` yang sama dengan `blip.toml`
- Secret token: kosongkan untuk signing token; boleh diisi sebagai fallback
- Trigger: centang **Push events**
- SSL verification: aktifkan bila memakai HTTPS

Gunakan **Test → Push events** terlebih dahulu. Respons yang diharapkan adalah `202 queued` bila branch payload `dev`, atau `204 ignored` untuk branch lain.

## 5. Uji alur merge

Buat Merge Request dari `feature/blip-webhook-mvp` ke `dev`, lalu merge. Pantau terminal Blip dan cek hasilnya:

```sh
cat .blip-test-deploy.log
cargo run -- --config blip.toml history blip-test
```

Harus ada satu eksekusi deploy setelah push ke `dev`. Untuk uji token salah, gunakan **Test** dengan signing token berbeda; respons harus `401 invalid webhook secret`. Timestamp webhook yang lebih tua dari 5 menit juga ditolak.

## Troubleshooting

- `connection refused`: proses Blip belum berjalan atau port tidak dapat dijangkau.
- `401`: signing token GitLab dan `signing_token` lokal berbeda.
- `204 ignored`: payload bukan branch `dev`; periksa konfigurasi atau event webhook.
- Tidak ada history: pastikan `scripts/test-deploy.sh` executable dan path script benar.
