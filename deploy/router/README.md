# Linux WAN/LAN router templates

These files are deployment templates, not an automatic network reconfiguration. Confirm the actual interface names with `ip -br link` before enabling them.

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
