# ChasselFi coin nodes

Coin mode accepts credit only from real hardware. A coin node may be an ESP32,
an Arduino with Wi-Fi/Ethernet, an Orange Pi, or a directly attached GPIO or
serial adapter.

## Network node design

Connect the node to the customer SSID/VLAN 799. DHCP gives it a `10.0.0.0/20`
address. It is deliberately **not** authenticated as a customer and receives no
general internet access. openNDS and the router allow only the local hardware
API at:

```
http://10.0.0.1:2081/api/coin-node/
```

Every request requires this header:

```
X-ChasselFi-Coin-Key: THE_SHARED_NODE_KEY
```

The installer generates the key once in `/etc/chasselfi/chasselfi.env` and
prints it during first installation. Read it later as root with:

```bash
grep '^CHASSELFI_COIN_NODE_KEY=' /etc/chasselfi/chasselfi.env
```

Use a different long random key if a node is lost. Restart ChasselFi after
changing it.

### Handshake

1. Every 10 seconds the node sends `POST /heartbeat`:

   ```json
   {"nodeId":"vendo-01","firmware":"esp32-1.0.0"}
   ```

2. The node polls `GET /status?nodeId=vendo-01` several times per second.
   `accepting` is `true` only while a customer has selected a package. Drive
   the coin acceptor enable/relay from that value. The response includes the
   current `claimId`.
3. For every debounced physical pulse, send `POST /pulse`:

   ```json
   {
     "nodeId":"vendo-01",
     "claimId":"CURRENT-CLAIM-UUID",
     "eventId":"boot123-pulse42",
     "count":1
   }
   ```

4. Retry a failed request with the **same eventId**. ChasselFi remembers event
   IDs for 24 hours, so a network retry cannot add the same coin twice.
5. A successful response explicitly returns `accepted: true`. When the package
   price is reached it also returns `completed: true`; immediately disable the
   acceptor. ChasselFi records the sale, creates/adds the paid session, and asks
   openNDS to authorize the customer.

The node must never enable its acceptor based only on Wi-Fi connectivity. It
must require `accepting: true`, a matching `claimId`, and a recent successful
heartbeat/status response. Fail closed when the server cannot be reached.

An ESP32 reference implementation is provided at
`hardware/esp32_chasselfi_coin_node/esp32_chasselfi_coin_node.ino`. It requires
the ArduinoJson library. Configure the SSID, password, shared key, pins, and
pulse electrical polarity before flashing.

## Directly attached adapter

A local GPIO/serial daemon can submit a debounced pulse through the restricted
Unix datagram socket:

```bash
/usr/local/libexec/chasselfi-coin-pulse 1
```

Only root and members of the `chasselfi` group can run the installed helper.
The runtime file `/run/chasselfi/coin-claim.json` exists only while the acceptor
should be enabled. A hardware daemon should use that file as its fail-closed
enable signal.

## Operator setup

In the admin dashboard open **Settings**, then choose:

- Voucher only
- Coin only
- Coin and voucher

Set **Value per hardware pulse** to the peso value emitted by the acceptor. A
normal one-peso-per-pulse device should remain set to `1`.

