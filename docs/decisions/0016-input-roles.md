# 0016 Input roles

Every input declares a role from a closed set: `selection`, `confirmation`, `calibration` or `model`. The schedule and the data rules read it. This record fixes how a row learns each input's role, and which roles a confirmation input may not share data with.

Decision:

- **The manifest.** The input view holds one more file at its top: `ignorarium:inputs.json`. It is canonical JSON (0002) with no final newline: an array with one `{"path", "role", "sha256"}` object per input in the view, in spec order. `sha256` is the input's store digest. An input absent from the view is not listed. The role stays required and closed: the box gives each role a meaning, so a payload cannot name one of its own.
- **Where it lives.** `prepare` puts the manifest's bytes into the content store, and the view links it to its blob as it links each input. The view writes the manifest itself, straight into its top folder. Every other entry is an input path, and every input path passes the path check, which refuses a `:`. So no input can take the manifest's name: the refusal is the guarantee, and the manifest never passes through it.
- **Records.** Each row record names the manifest's store digest under `manifest`. The store holds its bytes, so a reader can see which roles a row saw.
- **Checks.** The manifest is a view entry like any input. `prepare` hashes it with the rest, and a change to its blob before an attempt pauses the run (0006). The view is the same at levels 0, 1 and 2, so the manifest reaches every level unchanged.
- **The calibration pair.** E-CNF-004 now compares a confirmation input with both the selection and the calibration inputs. In a confirmatory plan, a confirmation input may not hold the data of either: not the same bytes, and not the same text in other case or spacing (the exact and the normalized hash, 0012). The detail names the first such input and its role. The rule stays confirmatory only, so an exploratory plan may calibrate on its confirmation text. `check` cannot compare an input that is absent at registration, so `prepare` makes the same comparison on every `run` and `resume`, and refuses with E-CNF-004's message before it writes the schedule. A path that names a folder is not an input's data: the box never reads or links a folder, so it shares nothing.

Why:

- Rows already get all their data through the view. A file there works at every level with no new channel and has no size limit.
- An environment variable does have one. `execve` takes one environment string of at most 32 pages, 128 KiB on 4 KiB pages. At about 140 bytes an entry, that holds under 1,000 inputs.
- A model calibrated on a text has adapted to it. Williams and Aletras quantized models with 4-bit GPTQ and changed only the calibration source. The WikiText perplexity moved by up to 0.18, as much as the quantization itself. Wikipedia calibration gave the lowest WikiText perplexity in 5 of 9 models. A model's error on its calibration text is not its error on new text.
- Eggensperger, Lindauer and Hutter keep the instances that score a configuration disjoint from the instances that tune it, because a score on tuning data is biased. Calibration fits a model to its text, as tuning does.

Cost: a payload must know one reserved name. The manifest's blob has no `import` record, so a published bundle that shows the roles a row saw must carry the blob.

Sources:

- https://man7.org/linux/man-pages/man2/execve.2.html (limits on the size of arguments and environment)
- Williams and Aletras (2024), "On the Impact of Calibration Data in Post-training Quantization and Pruning", https://arxiv.org/html/2311.09755v2
- Eggensperger, Lindauer and Hutter (2019), "Pitfalls and Best Practices in Algorithm Configuration", Journal of Artificial Intelligence Research 64, https://ar5iv.labs.arxiv.org/html/1705.06058
