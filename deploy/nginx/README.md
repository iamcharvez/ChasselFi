# Nginx deployment

1. Install Nginx and copy `chasselfi.conf` to `/etc/nginx/conf.d/`.
2. Configure ChasselFi to listen on `127.0.0.1:8080`.
3. Run `nginx -t` and reload Nginx.
4. Add TLS before exposing the admin portal outside a trusted LAN. Use your certificate manager of choice, then enable port 443 and set `CHASSELFI_SECURE_COOKIES=1`.
5. Restrict `/api` and the admin dashboard to the management subnet with your firewall. The customer portal should be exposed only on the captive LAN.

The login endpoint is rate-limited at the edge as a second layer in addition to ChasselFi's application-level throttling.
