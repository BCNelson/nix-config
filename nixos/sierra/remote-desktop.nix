_:

# Remote desktop into sierra's Plasma session from the phone, over Tailscale.
#
# The server is KRDP (kdePackages.krdp), which Plasma 6 already installs. It is
# a per-user server inside the running session: it shares the real desktop, so
# there must be a logged-in session (at the SDDM greeter there is nothing to
# connect to), and a locked screen shows up as the lock screen, which can be
# unlocked remotely.
#
# It's switched on and configured in System Settings -> Remote Desktop, not
# here. That page sets the RDP username and password (kept in KWallet), creates
# the TLS certificate and turns on autostart. The settings live in
# ~/.config/krdpserverrc.
#
# Port 3389 is open on tailscale0 only. The LAN and the wider internet can't
# reach it, and Tailscale ACLs decide which tailnet devices can.
{
  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ 3389 ];
}
