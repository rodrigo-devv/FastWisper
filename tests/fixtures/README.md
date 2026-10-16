# Fixtures de áudio

Áudio público, sem material privado. Gravações do usuário ficam fora do repositório.

| Arquivo | Idioma | Duração | Origem | Licença | SHA-256 |
|---|---|---|---|---|---|
| `en-jfk.wav` | en | 11,0 s | `samples/jfk.wav` do whisper.cpp na tag `v1.8.3` (`2eeeba56e9edd762b4b38467bab96c2517163158`). Discurso de posse de J. F. Kennedy (1961). | Domínio público; repositório MIT | `59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e` |
| `pt-casa-velha.wav` | pt | 10,0 s | LibriVox, *Casa Velha* (Machado de Assis), capítulo 2 (`casavelha_02_assis_64kb.mp3`, SHA-1 `57ef49c5e0950d31a1a6db3110aa85b3b9a579ee`), recorte de 40 s a 50 s. | Public Domain Mark 1.0 | `52f54b8cd7b79fbe3c0cd91c96940e6258969df5234422c23620fd3870baae2e` |

Recorte do pt, a partir do MP3 de `archive.org/download/casavelha_1504_librivox/`:

```bash
ffmpeg -ss 40 -t 10 -i casavelha_02_assis_64kb.mp3 -ac 1 -ar 16000 -c:a pcm_s16le pt-casa-velha.wav
```

Texto esperado (núcleo verificado nos testes, sem exigir pontuação exata):

- `en-jfk.wav`: "And so, my fellow Americans, ask not what your country can do for you, ask what you can do for your country."
- `pt-casa-velha.wav`: "… naquele estado. Vem cá, Lalau. Lalau aproximou-se de D. Antônia, que lhe compôs o cabeção do vestido; depois foi sentar-se defronte …"

Os testes com modelo real são `#[ignore]`: `cargo test -p fw-core --test asr_contract -- --ignored`.
