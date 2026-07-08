## Version reset: v1.1.0 → v0.1.0 (2026-07-08)

The fleet moved to pre-stable 0.x semantics (Powder landmark-016/017): versions below 1.0.0 use Cargo-style bumps (breaking→minor, feat/fix→patch) and never cross 1.0.0 automatically; promotion to 1.0.0 is a deliberate manual tag. v0.1.0 is the same commit as v1.1.0. Earlier 1.x/2.x entries below record real history under the old numbering; their tags and GitHub releases were retired.

# [1.1.0](https://github.com/misty-step/threshold/compare/v1.0.0...v1.1.0) (2026-07-01)


### Bug Fixes

* allow bb submission ids for verdict dispatch ([da039c1](https://github.com/misty-step/threshold/commit/da039c1daa19ad6aec0a8e58ce0d37c48ed9a7a4))
* enforce optimizer dispatch budget ([6542aa3](https://github.com/misty-step/threshold/commit/6542aa3aeb1e9c18bd7237e1244b49402864944b))
* fail closed on missing sprite verdicts ([21a120d](https://github.com/misty-step/threshold/commit/21a120d0d26ee9bd907459358418de2619e6f2cc))
* harden optimizer review findings ([97f744f](https://github.com/misty-step/threshold/commit/97f744fa86e52d0ef065399d1a2b16e0ade766e7))
* lift bb metering into sprite receipt ([270fd16](https://github.com/misty-step/threshold/commit/270fd162e4a528d0411e243bd9d18e406fe7c300))
* normalize Bitterblossom plane config path ([3683eb6](https://github.com/misty-step/threshold/commit/3683eb68c6b4540ea7d24b7b885e01b835c062d5))
* pass absolute payload path to bb run ([0bfa7be](https://github.com/misty-step/threshold/commit/0bfa7be123fa6e0b4d37399e22cdaaa5251d0b47))
* require oracle ceiling before saturated verdict ([584a287](https://github.com/misty-step/threshold/commit/584a287b6916a6112453748f919f11c9e2a3d7ce))
* score advisory optimizer verdicts ([687fc01](https://github.com/misty-step/threshold/commit/687fc01c27cb61f20eee119b03cc78da2eaf0aa7))


### Features

* add Crucible headroom optimizer probe ([0de7c6c](https://github.com/misty-step/threshold/commit/0de7c6c1b77603514b08794741caf6deb16bb77b))
* add Crucible optimizer loop ([3a44ee0](https://github.com/misty-step/threshold/commit/3a44ee02410a5583b482e618cf9145320eea3fe3))

# 1.0.0 (2026-07-01)


### Bug Fixes

* address CodeRabbit rename review ([f782db5](https://github.com/misty-step/threshold/commit/f782db5a8c3eae82b4246219e252ebb7de963857))
* address rename review findings ([03dfe53](https://github.com/misty-step/threshold/commit/03dfe53ad31c9f032d97789d05bddc13808f4d25))
* **cli:** reject negative/NaN --min-effect (039 child-2 review) ([c49c973](https://github.com/misty-step/threshold/commit/c49c9733b06b755d9dbeef44fa00295220140e27))
* **cli:** retire stale Python replay commands ([5b0c220](https://github.com/misty-step/threshold/commit/5b0c220629e48e80eac394e770e23792d892feef))
* **export:** make launch contracts portable ([c4a318d](https://github.com/misty-step/threshold/commit/c4a318d7871cc26aa648b7614b709b395dc2f04c))
* resolve remaining CodeRabbit link comment ([1cc20c8](https://github.com/misty-step/threshold/commit/1cc20c8267b3ef3f8cdee161216b6092b6e3af4e))
* **rust:** bound pi trial timeout and skip oversized one-shot probes ([6df1fd9](https://github.com/misty-step/threshold/commit/6df1fd9c7bcae307d371007f605507d94438832d))
* **rust:** make round_half_even bit-exact with Python round(); add battery oracle ([03f3eda](https://github.com/misty-step/threshold/commit/03f3eda47cb275611ee8352622fdb6acb8436edd))
* **rust:** one shared drain deadline in run_with_timeout; fork-proof kill test ([d4a21b7](https://github.com/misty-step/threshold/commit/d4a21b71efa42b63819ee6a5b27667e25f985e8b))
* **rust:** repo-root detection uses Cargo.toml, not the deleted runner/ ([7308236](https://github.com/misty-step/threshold/commit/7308236e9e0eb4f97ef2e7d8c5fe5e858d3e80ed))
* **stats:** compute significance from unrounded CI bounds ([4ea20e9](https://github.com/misty-step/threshold/commit/4ea20e9e86d9c310d9aac814c53299238f646f62))
* **stats:** refine t_975 fallback above df=30 (slice-A review) ([54b103d](https://github.com/misty-step/threshold/commit/54b103dea532e7977a3777e2e4907e1bb679907e))
* **stats:** t_{G−1} critical values for the reward-delta CI ([1fe51ad](https://github.com/misty-step/threshold/commit/1fe51adda6de4afff9edb90073f0945a9a0883c4))
* **workbench:** unique validate_arena scratch dir (slice-B review) ([c266ca6](https://github.com/misty-step/threshold/commit/c266ca6d0d81af2a1935c64790f98c95389e5e3f))


### Features

* **arena:** designate pr-review-v0 as the contamination-resistant holdout (043) ([f216151](https://github.com/misty-step/threshold/commit/f216151ba37f51a5e5d4ddab03bea46c79d9840c))
* **cerberus:** stand up + run the certified reviewer research loop ([ea3057b](https://github.com/misty-step/threshold/commit/ea3057b6f6de2265c560abcf631e648ebe3243b4))
* certify review swarm specialist baselines ([141849c](https://github.com/misty-step/threshold/commit/141849c4701ec81dcd7dbb960cb032fe940b30a8))
* certify synthetic review master baseline ([8de8071](https://github.com/misty-step/threshold/commit/8de80719840a59304b195a068ddc49f8e322bc45))
* **cli:** certify against incumbent baseline ([4e0ab6c](https://github.com/misty-step/threshold/commit/4e0ab6c22c4d1b75be62dd5439440849ca76c3b3))
* **cli:** operator trust & cost surface (estimate, compare, verdict) ([ffe04fb](https://github.com/misty-step/threshold/commit/ffe04fb52c24169381ea86a00d1dc151bae3cc27))
* **cli:** port bin/daedalus to Rust with clap + port run_oneshot ([7d5f800](https://github.com/misty-step/threshold/commit/7d5f800c7847247e4b1d03b7463d60c41bec5a56))
* **cli:** reliability gate — pass^k vetoes the recommendation (056) ([5b5b2ca](https://github.com/misty-step/threshold/commit/5b5b2ca0d361d58ab61fd4b242dd5cd02c611711))
* **core:** Rust validation kernel — one validator, not four dialects ([1d17831](https://github.com/misty-step/threshold/commit/1d1783163c71733779244eb816d228d4ec2e0e22))
* export cerberus reviewer config packet ([8161fab](https://github.com/misty-step/threshold/commit/8161fab5682825563f0d781b2f1caab1dfe121a3))
* export review swarm member inspection packet ([1b824c7](https://github.com/misty-step/threshold/commit/1b824c78df77e3318aeb3571d6dfe144cb346355))
* live `daedalus view` run dashboard [049] ([#22](https://github.com/misty-step/threshold/issues/22)) ([f96831b](https://github.com/misty-step/threshold/commit/f96831b43fb951c2314b01998cfd2a328395f16e))
* prepare review swarm suite gate packet ([6be6aa5](https://github.com/misty-step/threshold/commit/6be6aa5eb354255ac71d1b503c6207a73fdebac5))
* **report:** contamination + redteam-span advisory in the report ([28e7fbf](https://github.com/misty-step/threshold/commit/28e7fbff821a8a0e6ab09fc9a14c5cdcb3febfef))
* **roster:** latest-models-only policy + mechanical enforcement + observability epic ([e6758e7](https://github.com/misty-step/threshold/commit/e6758e720af2c1286168f0a02b08e17801fbba9b))
* run correctness autoresearch v0.2 ([d69fe43](https://github.com/misty-step/threshold/commit/d69fe43b21bd6e5e09d9b4f7751be929302b0a8b))
* **rust:** add pycompat helper; port prompt_packet (Tier 1) ([115c73b](https://github.com/misty-step/threshold/commit/115c73b29cbed5fef13c03ba249d91de1bd0584b))
* **rust:** add pycompat::days_from_civil for date arithmetic (doctor) ([e4a7762](https://github.com/misty-step/threshold/commit/e4a77620c78e81c612030abb0f4661993023092e))
* **rust:** add pycompat::py_json_dumps (Python json.dumps fidelity) ([9a6ece1](https://github.com/misty-step/threshold/commit/9a6ece127797eb148db9319178ca04c4a630a3d0))
* **rust:** add pyrandom — CPython-exact MT19937 (shuffle/getrandbits) ([f7db828](https://github.com/misty-step/threshold/commit/f7db8283b1d35b01d5b0bd33be8dea5618b7a31f))
* **rust:** port doctor (Layer 0) ([7efc855](https://github.com/misty-step/threshold/commit/7efc85506e73cdffac9f449e8e39f5bc2d889bce))
* **rust:** port export (Layer 2) ([1ed23de](https://github.com/misty-step/threshold/commit/1ed23defcff07c517000318d344b42a5b669d7af))
* **rust:** port judge (Tier 1) ([15d9faf](https://github.com/misty-step/threshold/commit/15d9faffb48c10a616bcd22a90dddcafc8de125e))
* **rust:** port launch (Layer 1) ([f849bec](https://github.com/misty-step/threshold/commit/f849bec0329e919e806a814c1285d94e671094a1))
* **rust:** port lineage (Tier 1) ([049831a](https://github.com/misty-step/threshold/commit/049831af58e4a918efe199d735348a8fdfb451ea))
* **rust:** port loop (search core) on CPython-exact PyRandom ([46aaf2d](https://github.com/misty-step/threshold/commit/46aaf2d5fcfa55a0cc6a78ada499d5eb4fe8203c))
* **rust:** port mutate deterministic core (Layer 1) ([5e03528](https://github.com/misty-step/threshold/commit/5e035281220ba63393590e2d2e67b9f8c8fa7719))
* **rust:** port port_harbor (Tier 1) ([9625d31](https://github.com/misty-step/threshold/commit/9625d31d9684767c3eed03e37cd35f3b78f6fd1a))
* **rust:** port report (Tier 1) ([f6b42cd](https://github.com/misty-step/threshold/commit/f6b42cd67f14becce18520d281aec4003c00d6cb))
* **rust:** port run deterministic core (Tier 3 prep) ([8479143](https://github.com/misty-step/threshold/commit/8479143d8e65f366ea017fc8804d5e4a878e234b))
* **rust:** port seed deterministic core (Layer 2) ([c18cf11](https://github.com/misty-step/threshold/commit/c18cf119c035eae66a0df1a617d861e6697ddf7d))
* **rust:** port swarm (Layer 0) ([adef376](https://github.com/misty-step/threshold/commit/adef376ec38182f91f3e4df1950b8060013018cd))
* **rust:** port taxonomy (Tier 1) ([d79cd05](https://github.com/misty-step/threshold/commit/d79cd05345c0f98e41e232204e0a12ad52710005))
* **rust:** port trace; enable preserve_order; add pycompat truthiness/str ([2ad875b](https://github.com/misty-step/threshold/commit/2ad875b7fc1b5830fc9bb831df6c8f183cd07a10))
* **rust:** port workbench (Layer 1) ([4b0a6b1](https://github.com/misty-step/threshold/commit/4b0a6b165bf33786046e6dd0f31fca9c6d0c8175))
* **rust:** run execution glue (run_pi/run_arena) + null/oracle e2e parity ([5ee91a4](https://github.com/misty-step/threshold/commit/5ee91a483dde9ba16613e9074084e4acc36fae86))
* **rust:** scaffold workspace; port the scorer behind a Python parity oracle ([5505680](https://github.com/misty-step/threshold/commit/55056804d8fef0501d8baa53167961a077c9db95))
* scaffold optional review swarm lenses ([6958b95](https://github.com/misty-step/threshold/commit/6958b95aa3e7de5c1c829d7a76502b42e64f9b7b))
* **score:** red-team audit for gameable answer keys (040 slice C) ([6a42d3f](https://github.com/misty-step/threshold/commit/6a42d3f11af9644d0bb13cd7c6b4d980e0e6ac78))
* self-contained HTML run reports (report-html) [044] ([#21](https://github.com/misty-step/threshold/issues/21)) ([fd54ba1](https://github.com/misty-step/threshold/commit/fd54ba11cc8d1848bede799cd06d7a5426ce05a8))
* source_repo labels activate repo-clustering (040 slice A) ([95b296a](https://github.com/misty-step/threshold/commit/95b296a1c4a2a1fa89827fbe746830bbbb3fcae0))
* **stats:** basin-trap detector across seed trajectories (039 child-4) ([76118d5](https://github.com/misty-step/threshold/commit/76118d56404ac9c5839206a07723f36a93a30790))
* **stats:** cluster-robust 95% CIs on reward deltas (039 child-1) ([e5176a5](https://github.com/misty-step/threshold/commit/e5176a534e8aea349bd8d541cb2584d32f692fb2))
* **stats:** gate certification on significance (039 child-2) ([178323c](https://github.com/misty-step/threshold/commit/178323c19b7624d2ae8166773ca08b0f6fe74ebe))
* **stats:** per-candidate pass^k reliability metric (039 child-3) ([8e90d2c](https://github.com/misty-step/threshold/commit/8e90d2c4694224327303543c7f38962d7729a029))
* **stats:** power note — min clusters to certify the observed effect (039 child-5) ([8c675de](https://github.com/misty-step/threshold/commit/8c675de0bf32082ad02974aab9ce0dc0bd418b5e))
* **ui-lab:** daedalus operator UI design lab — the instrument ([d65b589](https://github.com/misty-step/threshold/commit/d65b589bec6d7097f1476590ecaf246b90aa072e))
* **workbench:** add arena freeze command ([f6c9896](https://github.com/misty-step/threshold/commit/f6c989665385e90afafba5ce2b27400817b826ed))
* **workbench:** designate contamination-resistant holdout (040 slice E) ([6afe970](https://github.com/misty-step/threshold/commit/6afe970990c6727399e190838b0d8f0fc5ff1ae0))
* **workbench:** machine-readable contamination records (040 slice D) ([47d4fa6](https://github.com/misty-step/threshold/commit/47d4fa655db2bd21cbb8800ab7a2c949c7978d46))
* **workbench:** saturation-probe error guard (040 slice B) ([584e05a](https://github.com/misty-step/threshold/commit/584e05a4446c6499ea821551db5ade5dddb89267))
