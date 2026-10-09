# koshnitsa

An unofficial [MCP](https://modelcontextprotocol.io) server for the [ebag.bg](https://ebag.bg) online
supermarket, written in Rust. It lets an AI assistant such as Claude search the catalogue, fill and
edit your cart, read your past orders and shopping lists, and check delivery slots.

*Кошница* is Bulgarian for basket.

> [!WARNING]
> **This is an experimental, personal project. Use it entirely on your own accord and at your own
> responsibility.**
>
> - It is **not affiliated with, endorsed by or supported by eBag** or Кънвиниънс АД.
> - It talks to eBag's **private, undocumented** web API, which can change or break at any moment
>   without notice. When it does, this tool will misbehave until someone fixes it.
> - Automated access may be contrary to eBag's terms of service. Read
>   [eBag's Общи условия](https://ebag.bg/terms) and decide for yourself before using it. Any
>   consequence for your account is yours.
> - An AI agent acting on your cart can make mistakes. Check your cart before you order.
> - It is provided "as is", without warranty of any kind. See the licence.

## What it does

It covers the whole flow, from search to a placed order. Ordering is **off until you turn it on**,
and even then the server asks you to confirm every order itself. With ordering off, the agent fills
the cart and you finish in the eBag app.

| Tool | What it does | Needs login |
|---|---|---|
| `search_products` | Catalogue search with EUR price, unit price, pack size, availability | no |
| `get_product` | Detail: promotion, restock date, origin, nutrition, short description | no |
| `list_delivery_slots` | Delivery windows for the coming days and how full they are | no |
| `get_cart` | Current cart lines and eBag's totals | yes |
| `add_to_cart` | Add up to 40 products in one call | yes |
| `set_cart_quantity` | Set a line to an exact quantity; 0 removes it | yes |
| `list_orders` / `get_order` | Order history and the items in an order | yes |
| `reorder` | Copy a past order's items into the cart (does not order) | yes |
| `list_shopping_lists` / `add_to_shopping_list` | Saved lists | yes |
| `checkout_summary` | Cart, out-of-stock lines and the next free slots | yes |
| `list_addresses` | Saved delivery addresses | yes |
| `prepare_order` | Runs eBag's checkout up to its final review and returns eBag's own summary and a token. Orders nothing | yes |
| `place_order` | Places the prepared order, after you confirm it. Off by default | yes |

Safety rails, enforced in the server rather than left to the agent:

- `place_order` refuses unless ordering is enabled in the config file, which the agent cannot
  change.
- It needs a token from `prepare_order` that works once and expires after ten minutes.
- Before submitting, it asks eBag to review the order again. If anything has changed since you saw
  the summary (cart, stock or prices), eBag's review hash changes and the server refuses.
- It refuses totals over your cap and any payment method you have not allowed. Only pay-on-delivery
  methods exist, so no card is charged online and 3-D Secure never comes up.
- It asks you directly through MCP elicitation and shows eBag's own figures, not the model's
  summary. If your client cannot show that prompt, it refuses.
- Every prepared, declined, placed or failed order is appended to `~/.config/koshnitsa/orders.log`.
- Quantities are capped at 50 per line and 40 lines per call.
- Requests are serialised and spaced at least 300 ms apart.
- Product descriptions are stripped of HTML and cut to 600 characters, and the server tells the
  model that supplier text is data, not instructions.

## Install

Requires a recent stable Rust toolchain.

```sh
cargo install --git https://github.com/cargopete/koshnitsa koshnitsa-mcp
```

This installs a binary called `koshnitsa`.

## Log in

eBag has no API login, so koshnitsa reuses your browser session. If you are logged in at
[ebag.bg](https://ebag.bg) in Chrome, Brave, Arc, Edge, Firefox or Safari, the simplest way is:

```sh
koshnitsa login --browser chrome
```

This reads the ebag.bg cookies from the browser's own cookie store. On macOS, Chromium browsers
ask for Keychain access the first time, and Safari needs your terminal to have Full Disk Access.

Or copy the cookie by hand:

1. Log in at [ebag.bg](https://ebag.bg) in your browser.
2. Open DevTools → Network, click any request to `ebag.bg`, and copy the **Cookie** request header.
3. Run `koshnitsa login`, paste it, and press Ctrl-D.

The cookie is checked against eBag and stored in your OS keychain (macOS Keychain, Windows
Credential Manager, or the Secret Service on Linux). `koshnitsa status` checks it is still valid and
`koshnitsa logout` removes it. When the session expires, tools say so; log in again.

The `EBAG_COOKIE` environment variable overrides the keychain if set.

Search, product details and delivery slots work without logging in.

## Connect it

**Claude Code**

```sh
claude mcp add koshnitsa -- koshnitsa
```

**Claude Desktop**, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "koshnitsa": { "command": "koshnitsa" }
  }
}
```

Use the full path to the binary (`which koshnitsa`) if Claude Desktop cannot find it.

It runs locally over stdio. Your cookie never leaves your machine except to go to ebag.bg.

## Enabling ordering

Create `~/.config/koshnitsa/config.toml`:

```toml
[ordering]
enabled = true
max_total_eur = 80                       # refuse anything above this, delivery and tip included
allowed_payment_methods = ["cash"]       # or "card_on_delivery"
require_confirmation = true              # ask you before every order; leave this on
```

Restart the MCP client afterwards. The agent can read the config but has no tool that changes it.

## Status

What has been checked, and what has not:

- **Verified end to end against live ebag.bg with a logged-in account:** search, product detail,
  slots, cart read, add and remove, order history and detail, shopping lists and
  `checkout_summary`.
- **Checkout:** the address, slot and tip requests were captured from a real checkout session.
  The payment, review and order requests, and the payment method ids (cash `3`, card on delivery
  `11`), come from eBag's own frontend code. `prepare_order` has been run live up to the payment
  step. **`place_order` has never placed a real order.** The first one will be its test, so keep
  the cap low and check the eBag app afterwards.
- **Not yet exercised live:** `reorder` and `add_to_shopping_list`. Reports welcome.
- Not handled: login by email and password, online card payment, the mobile app's API.

## Development

```sh
cargo test --workspace
```

The tests run against eBag responses captured into `crates/koshnitsa-client/tests/fixtures/` and
a local mock server; they never contact ebag.bg.

- `crates/koshnitsa-client`: the eBag HTTP client and the trimmed models handed to the agent.
- `crates/koshnitsa-mcp`: the MCP server and the `login`/`status`/`logout` commands, on
  [rmcp](https://github.com/modelcontextprotocol/rust-sdk).

## Credits

The endpoint map started from [nb/ebag](https://github.com/nb/ebag), an unofficial eBag CLI. No
code was copied from it.

## Licence

MIT or Apache-2.0, at your option.
