# Muse Code

The native Rust/GPUI app can run the official Muse Code CLI over ACP v1.
Install Muse Code using [Meta's instructions](https://dev.meta.ai/products/muse-code/),
then run `muse login` in a terminal. Restart Monocode after installing the CLI.
Choose **Muse Code** in the model picker or Settings → Providers.

Monocode launches `muse acp` with `MUSE_EXPERIMENTAL_ACP=on` and uses the
CLI's existing login. Models and reasoning controls come from the CLI's catalog;
no API key or fixed model list is stored by this integration. Muse 1.4's ACP
menu omits effort tiers without descriptions, so discovery reads the complete
`model/list.variants` list over MSP (`muse serve`). Before a turn with a selected
effort, a short-lived MSP host creates or resumes the session and durably applies
`session/setReasoningEffort`; ACP then opens that same session. Muse processes
are released after each turn so effort changes can acquire the session lease.
No global Muse settings are changed. Normal conversation
processes use `--trust-workspace` to load the project's rules and skills.

Supervised and Auto select Muse's `promptUnmatched` approval mode. Auto keeps
command approval prompts because Muse does not advertise Monocode's automatic
reviewer. Auto-accept edits also selects `promptUnmatched`, and Monocode answers
eligible file-operation approval requests. Full access selects `allowAll`.
Muse permission rules can allow actions without prompting, as in its own CLI.
Read-only and Plan return an explicit unsupported-mode error before sending a
prompt. Background text jobs disable writes, shell execution, and questions and
use `denyUnmatched`; discovery also disables writes and shell execution.

Existing ACP handling supplies streamed replies, tools, approvals, cancellation,
saved-session loading, attachments, and `/compact`. Forms are not advertised;
answer Muse's questions in a subsequent message when it cannot display a form.
Keep a session closed in other Muse processes before resuming it in Monocode.

This integration targets the native local provider. The React/Tauri app and
the TypeScript remote host do not register Muse. The unofficial
[Muse Spark Code adapter](https://github.com/RandyNorthrup/muse-spark-code)
is a separate agent and is not needed here.

Protocol reference: [Muse Code 1.4 editor integration](https://meta-models.github.io/muse-code-sdk/1.4/guides/editors-acp/).

Run the native app with `npm run native:dev`. To check discovery and a two-turn
conversation using your existing Muse login:

```sh
cargo run --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-core --profile native-dev --example provider_smoke -- muse --models
cargo run --locked --manifest-path crates/monocode-gpui/Cargo.toml -p monocode-core --profile native-dev --example provider_smoke -- muse
```

The conversation check consumes Muse account usage. Visual verification of the
picker, transcript, and approval dialogs must be done in the running native app.
