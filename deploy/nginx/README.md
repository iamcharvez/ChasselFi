# Nginx deployment

1. Install Nginx and copy `chasselfi.conf` to `/etc/nginx/conf.d/`.
2. Configure ChasselFi to listen on `127.0.0.1:8080`.
3. Run `nginx -t` and reload Nginx.
4. Add TLS before exposing the admin portal outside a trusted LAN. Use your certificate manager of choice, then enable port 443 and set `CHASSELFI_SECURE_COOKIES=1`.
5. The admin console is available on WAN/non-LAN addresses. On `10.0.0.1` (VLAN 799), only the customer portal routes are allowed; admin routes return `404`.
6. Keep the WAN management address restricted with your firewall and add TLS before exposing it beyond a trusted management network. The IPv6 listener is intentionally omitted so IPv6 clients cannot bypass the VLAN 799 policy.

The login endpoint is rate-limited at the edge as a second layer in addition to ChasselFi's application-level throttling.
