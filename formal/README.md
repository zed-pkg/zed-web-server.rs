# Browser mutation verification

This repository is an additional formal-methods consumer alongside Zed API,
sync, and lock. Its model covers one configured browser mutation request, not
the whole authentication service or registry. The API remains the authority
for membership, namespace creation, transactionality, and authorization.

## Executable boundary

`browser_mutation.qnt` has 72 initial input combinations: two origin outcomes,
two session outcomes, three refresh outcomes (same principal, rejected, changed
principal), two delegation outcomes, and three API outcomes (success, rejection,
redirect). The finite model exhaustively checks:

- no refresh without same-origin browser authority and a valid session;
- no delegation after a rejected refresh or principal change;
- no API mutation without successful scoped delegation;
- only API success yields `Applied`, never an authentication/API redirect;
- successful same-principal refresh rotation survives terminal failures;
- a returned cookie claims recent verification only after successful delegation.

The model's `Phase` and `Outcome` are closed variants. Rust uses the exhaustive
`BrowserMutation::{Applied, SignIn, Failed}` match at the presentation boundary;
no caller infers successful creation from an HTTP status code. HTTP 303 remains
correct for the POST-to-GET sign-in transition.

## Implementation conformance

`src/browser_auth/mutation_conformance.rs` executes the production async BFF
against loopback Shared Auth/API servers for all 72 input combinations. It
checks the exact delegated audience/scopes, canonical mutation fields, calls
made, typed outcome, and the signed same-principal rotated cookie. Additional
tests reject expired/future/extreme issuance timestamps across subject lookup,
mutations, delegated reads, and continuity probes. The actual session-status
route is tested through a post-refresh delegation outage.

Sixteen seeded Quint ITF traces are also replayed through that same production
handler. Replay compares the model's terminal outcome, refresh/delegate/API
call observations, cookie presence, and delegation evidence. This is terminal-observation refinement,
not a proof of every internal Rust instruction or every possible network trace.
The separate complete input matrix prevents the small generated corpus from
being the only implementation coverage.

`replay.mjs --prepare` builds a normal Rust test executable before entering
fmctl's isolated environment. The adapter binds it to source and binary SHA-256
hashes, rejects changed source/binary, bounds input and execution, constrains
trace paths to artifacts, and requires exactly one replay test to execute.
An absent corpus, missing field, unknown variant, or unfinished trace fails.

`negative-controls.mjs` deliberately drops the rotated cookie in a derived
model and requires a TLC invariant counterexample. It also contradicts a
generated trace's terminal cookie observation and requires the real Rust
adapter to reject it. Tool startup/configuration failure is not accepted as
evidence that these negative controls work.

## Run the same gate locally and in CI

```sh
cargo test --locked --lib mutation_conformance
node formal/check.mjs /absolute/path/to/fmctl
```

`formal/fm.toml` owns the pinned Quint version, model, properties, witnesses,
trace seed/count, timeouts, and output limits. The script runs validate, check,
simulate, exhaustive TLC verify, trace, Rust replay, and the negative controls.
Ordinary Rust tests deliberately mark the generated-trace entry point ignored;
the formal CI job runs it explicitly after generating its mandatory corpus.

The standalone runner was tested at
`ORESoftware/formal-methods.rs@734a1c3ccb1c06181ac9c18cb75bbb8561b59891`.
That repository is private and this consumer has no cross-repository credential.
Hosted CI therefore pins the same public incubator runner already used by Zed
API: `opto-sync/opto-sync-clients@c2146ef9f054d24e1488c216547852aa148285cf`.
Both runners consume the same schema-v1 and batch-adapter contract; local checks
must exercise the public pin as well when changing this gate. Do not replace
either immutable pin with a moving branch or silently skip hosted verification.

## Limits and evidence

TLC explores the entire declared finite state graph. Simulation provides
non-vacuous reachability evidence, not the exhaustive proof. The initial checked
graph had 207 distinct states and depth 6. Counts are evidence, not hard-coded
expectations that would prohibit legitimate model evolution.

Cryptography, external authorization, database atomicity, multi-tab concurrent
refresh rotation, process death, response loss, and arbitrary network behavior
are outside this model. A signed cookie is not proof of product authorization;
the API rechecks that authority. No bearer token enters a browser error body,
formal trace, or test report. The fake loopback credentials are not secrets.

Reports, traces, and negative-control counterexamples remain in
`.formal-artifacts/`, excluded from Git and uploaded by CI. Upstream verifier
warnings remain in those artifacts and must not be confused with product
verification results. The platform-wide audit remains tracked in DEN-3970 and
`zed-pkg/.github#61`; this model does not close that umbrella.
