# Linux WAN/LAN router templates

These files are deployment templates, not an automatic network reconfiguration. Confirm the actual interface names with `ip -br link` before enabling them.

## VLAN 799 router and captive portal

For a single tagged trunk carrying the WAN as the parent interface and the
customer LAN as VLAN 799:

```bash
chmod +x deploy/router/*.sh
sudo bash deploy/router/install-vendo-vlan799.sh --wan enp2s0f0
```

The combined installer builds ChasselFi and configures VLAN 799,
`10.0.0.1/20`, DHCP, forwarding, NAT, Nginx, and openNDS. Its Forwarding
Authentication Service (FAS) is served internally at
`http://10.0.0.1:2080/portal/fas`; customers only need to browse
`http://10.0.0.1/`. The scripts back up files they replace.

Do not run the openNDS script until a VLAN client can obtain DHCP and the
server's routing has been tested. The switch port facing the server must be a
trunk with VLAN 799 tagged; customer/access-point ports must be access ports
in VLAN 799.

The admin dashboard is intentionally unavailable on the customer address. Use
`http://<WAN-IP>/admin/` from the management/WAN network.

The installer creates `CHASSELFI_FAS_KEY` in
`/etc/chasselfi/chasselfi.env`. Keep this value private and use the same
ChasselFi installation for the FAS endpoint. HTTPS should be added before
exposing the portal or administration surface outside the local network.

It also generates `CHASSELFI_COIN_NODE_KEY`. Network coin nodes join VLAN 799
but receive no internet bypass; openNDS permits only their authenticated local
API on `10.0.0.1:2081`. Follow `docs/COIN_NODES.md` before connecting the coin
acceptor relay or pulse wire.

Current VLAN 799 layout:

- `WAN_IF`: physical Ethernet connected to a managed-switch trunk; untagged WAN
- `LAN_IF`: `${WAN_IF}.799`, the tagged customer VLAN on the same port
- LAN gateway: `10.0.0.1/20`
- DHCP pool: `10.0.0.100` through `10.0.15.250`

Apply in this order on a maintenance console:

1. Configure the LAN address and verify you can reach `10.0.0.1` locally.
2. Enable forwarding and install the reviewed `nftables` rules.
3. Start `dnsmasq` with the LAN interface configuration.
4. Test a single client for DHCP, gateway ping, DNS, and internet access.
5. Only then enable ChasselFi's live router adapter.

Do not paste these templates unchanged into a production firewall. Replace the interface names and review every rule first.
