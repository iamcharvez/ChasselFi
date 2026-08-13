# Linux WAN/LAN router templates

These files are deployment templates, not an automatic network reconfiguration. Confirm the actual interface names with `ip -br link` before enabling them.

## VLAN 4001 router and captive portal

For a single tagged trunk carrying the WAN as the parent interface and the
customer LAN as VLAN 4001:

```bash
chmod +x deploy/router/setup-vlan4001.sh deploy/router/setup-opennds.sh
bash deploy/router/setup-vlan4001.sh --wan enp2s0f0
bash deploy/router/setup-opennds.sh --lan enp2s0f0.4001
```

The first script configures the VLAN, `10.0.0.1/20`, DHCP, forwarding, and
NAT. The second installs openNDS and points its Forwarding Authentication
Service (FAS) at `/portal/fas`, where ChasselFi validates vouchers and returns
the openNDS authentication token. The scripts back up files they replace.

Do not run the openNDS script until a VLAN client can obtain DHCP and the
server's routing has been tested. The switch port facing the server must be a
trunk with VLAN 4001 tagged; customer/access-point ports must be access ports
in VLAN 4001.

The installer creates `CHASSELFI_FAS_KEY` in
`/etc/chasselfi/chasselfi.env`. Keep this value private and use the same
ChasselFi installation for the FAS endpoint. HTTPS should be added before
exposing the portal or administration surface outside the local network.

Recommended layout:

- `WAN_IF`: built-in Ethernet connected to the upstream router/ISP (DHCP)
- `LAN_IF`: USB-to-LAN connected to the switch/access point
- LAN gateway: `10.0.0.1/20`
- DHCP pool: `10.0.0.10` through `10.15.250`

Apply in this order on a maintenance console:

1. Configure the LAN address and verify you can reach `10.0.0.1` locally.
2. Enable forwarding and install the reviewed `nftables` rules.
3. Start `dnsmasq` with the LAN interface configuration.
4. Test a single client for DHCP, gateway ping, DNS, and internet access.
5. Only then enable ChasselFi's live router adapter.

Do not paste these templates unchanged into a production firewall. Replace the interface names and review every rule first.
