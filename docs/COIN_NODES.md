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

Pair every network controller from **Admin → Coin nodes → Pair coin node**.
ChasselFi displays a node ID and a unique 40-character key exactly once. Store
both values in the controller firmware. The server stores only the key hash,
so losing the displayed key requires unpairing and pairing that node again.

Every request requires the generated key in this header:

```
X-ChasselFi-Coin-Key: THE_NODE_KEY
```

The older installation-wide key in `/etc/chasselfi/chasselfi.env` remains a
compatibility fallback for already-deployed firmware. New installations
should use a separately paired key for every node so one lost controller can
be revoked without changing every device.

To inspect the legacy fallback key as root:

```bash
grep '^CHASSELFI_COIN_NODE_KEY=' /etc/chasselfi/chasselfi.env
```

Do not embed the administrator password or FAS key in a controller. A paired
node key authorizes only the coin-node API and never grants WAN forwarding.

### Handshake

1. Configure the copied values in firmware:

   ```text
   NODE_ID=vendo-front-01
   COIN_NODE_KEY=the-one-time-key
   SERVER=http://10.0.0.1:2081
   ```

2. Every 10 seconds the node sends `POST /heartbeat`:

   ```json
   {"nodeId":"vendo-01","firmware":"esp32-1.0.0"}
   ```

3. The node polls `GET /status?nodeId=vendo-01` several times per second.
   `accepting` is `true` only while a customer has selected a package. Drive
   the coin acceptor enable/relay from that value. The response includes the
   current `claimId`.
4. For every debounced physical pulse, send `POST /pulse`:

   ```json
   {
     "nodeId":"vendo-01",
     "claimId":"CURRENT-CLAIM-UUID",
     "eventId":"boot123-pulse42",
     "count":1
   }
   ```

5. Retry a failed request with the **same eventId**. ChasselFi remembers event
   IDs for 24 hours, so a network retry cannot add the same coin twice.
6. A successful response explicitly returns `accepted: true`. When the package
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

In the admin dashboard:

1. Open **Coin nodes**, select **Pair coin node**, and securely copy the
   one-time firmware values.
2. Open **Settings**, then choose:

- Voucher only
- Coin only
- Coin and voucher

3. Set **Value per hardware pulse** to the peso value emitted by the acceptor. A
normal one-peso-per-pulse device should remain set to `1`.
4. Confirm the node becomes **Online** within 45 seconds before accepting real
   coins. Unpairing immediately revokes its current key.
