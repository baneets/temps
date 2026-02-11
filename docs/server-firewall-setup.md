# Server Firewall Setup Guide (iptables)

This guide provides a safe, stateful firewall configuration for Linux servers that allows:
- SSH access (port 22)
- HTTP/HTTPS traffic (ports 80, 443)
- Custom application ports
- Outbound connections with return traffic

## Prerequisites

- Root or sudo access to the server
- SSH access to the server
- Hetzner Console access as backup (or equivalent out-of-band access)

## Important: Disable Cloud Firewall First

If using Hetzner Cloud Firewall (or similar), either:
1. **Remove** the cloud firewall entirely, OR
2. Set it to **Allow All** incoming

Cloud firewalls are typically **stateless** and will block return traffic from outbound connections.

---

## Quick Setup (Copy-Paste Safe)

### Step 1: Create Safety Net

```bash
# Create automatic revert script (5 minute safety net)
nohup bash -c 'sleep 300 && iptables -F && iptables -P INPUT ACCEPT && iptables -P FORWARD ACCEPT && iptables -P OUTPUT ACCEPT && echo "Firewall reset at $(date)" >> /tmp/firewall-reset.log' &
echo "Safety net active - firewall will reset in 5 minutes if not cancelled"
```

### Step 2: Apply Rules (SSH First!)

```bash
# Allow SSH (MUST BE FIRST!)
iptables -A INPUT -p tcp --dport 22 -j ACCEPT

# Allow established/related connections (return traffic for outbound)
iptables -A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT

# Allow loopback interface
iptables -A INPUT -i lo -j ACCEPT

# Allow HTTP/HTTPS
iptables -A INPUT -p tcp --dport 80 -j ACCEPT
iptables -A INPUT -p tcp --dport 443 -j ACCEPT

# Allow custom application port (adjust as needed)
iptables -A INPUT -p tcp --dport 8081 -j ACCEPT

# Allow ICMP (ping) - optional but useful for diagnostics
iptables -A INPUT -p icmp --icmp-type echo-request -j ACCEPT
```

### Step 3: Test SSH Before Applying DROP

**⚠️ CRITICAL: Open a NEW terminal and verify SSH works before continuing!**

```bash
# In a NEW terminal window:
ssh user@your-server-ip

# If SSH works, continue. If not, wait 5 minutes for auto-reset.
```

### Step 4: Apply DROP Policy

```bash
# Only run this AFTER confirming SSH works in new terminal!
iptables -P INPUT DROP
iptables -P FORWARD DROP
```

### Step 5: Cancel Safety Net & Save Rules

```bash
# Cancel the auto-reset
pkill -f "sleep 300"

# Verify rules
iptables -L -v -n

# Save rules permanently
# Debian/Ubuntu:
apt install -y iptables-persistent
netfilter-persistent save

# RHEL/CentOS/Rocky:
service iptables save
# or
iptables-save > /etc/sysconfig/iptables
```

---

## Complete Rules File

For use with `iptables-restore` or `iptables-persistent`:

```bash
# /etc/iptables/rules.v4
*filter
:INPUT DROP [0:0]
:FORWARD DROP [0:0]
:OUTPUT ACCEPT [0:0]

# Allow loopback
-A INPUT -i lo -j ACCEPT

# Allow established/related (CRITICAL for outbound return traffic)
-A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT

# Allow SSH
-A INPUT -p tcp --dport 22 -j ACCEPT

# Allow HTTP/HTTPS
-A INPUT -p tcp --dport 80 -j ACCEPT
-A INPUT -p tcp --dport 443 -j ACCEPT

# Allow custom app port
-A INPUT -p tcp --dport 8081 -j ACCEPT

# Allow ICMP (ping)
-A INPUT -p icmp --icmp-type echo-request -j ACCEPT

# Log dropped packets (optional - can fill logs)
# -A INPUT -j LOG --log-prefix "IPTables-Dropped: " --log-level 4

COMMIT
```

### Apply from file (with auto-rollback):

```bash
# Apply with 30-second confirmation timeout
iptables-apply -t 30 /etc/iptables/rules.v4
```

---

## IPv6 Rules (Optional)

If your server has IPv6, create matching rules:

```bash
# /etc/iptables/rules.v6
*filter
:INPUT DROP [0:0]
:FORWARD DROP [0:0]
:OUTPUT ACCEPT [0:0]

-A INPUT -i lo -j ACCEPT
-A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
-A INPUT -p tcp --dport 22 -j ACCEPT
-A INPUT -p tcp --dport 80 -j ACCEPT
-A INPUT -p tcp --dport 443 -j ACCEPT
-A INPUT -p tcp --dport 8081 -j ACCEPT
-A INPUT -p ipv6-icmp -j ACCEPT

COMMIT
```

---

## Recovery Procedures

### If Locked Out

| Method | Steps |
|--------|-------|
| **Wait for auto-reset** | If safety script running, wait 5 minutes |
| **Hetzner Console** | Cloud Console → Server → Console tab (VNC) |
| **Rescue Mode** | Cloud Console → Server → Rescue → Enable → Reboot |

### From Rescue Mode

```bash
# Mount root filesystem
mount /dev/sda1 /mnt  # adjust device as needed

# Clear iptables rules file
echo "" > /mnt/etc/iptables/rules.v4

# Reboot
reboot
```

### Emergency Reset Command (if still connected)

```bash
iptables -F && iptables -P INPUT ACCEPT && iptables -P FORWARD ACCEPT && iptables -P OUTPUT ACCEPT
```

---

## Verification Commands

```bash
# View current rules with packet counts
iptables -L -v -n

# View rules in save format
iptables-save

# Check if conntrack module loaded
lsmod | grep conntrack

# Test outbound connectivity
curl -I https://google.com

# Check listening ports
ss -tlnp
```

---

## Adding New Ports

```bash
# Add new port (example: 3000 for Node.js)
iptables -I INPUT 6 -p tcp --dport 3000 -j ACCEPT

# Save after adding
netfilter-persistent save
```

---

## Common Port Reference

| Service | Port | Rule |
|---------|------|------|
| SSH | 22 | `-A INPUT -p tcp --dport 22 -j ACCEPT` |
| HTTP | 80 | `-A INPUT -p tcp --dport 80 -j ACCEPT` |
| HTTPS | 443 | `-A INPUT -p tcp --dport 443 -j ACCEPT` |
| PostgreSQL | 5432 | `-A INPUT -p tcp --dport 5432 -j ACCEPT` |
| Redis | 6379 | `-A INPUT -p tcp --dport 6379 -j ACCEPT` |
| Node.js | 3000 | `-A INPUT -p tcp --dport 3000 -j ACCEPT` |

---

## Why This Works

The key rule is:
```bash
-A INPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
```

This enables **stateful** firewall behavior:
- Tracks all outbound connections
- Automatically allows return packets for those connections
- No need to open ephemeral ports (32768-65535)
- Secure: only allows responses to connections YOU initiated

---

## Checklist Before Leaving Server

- [ ] SSH tested in separate terminal
- [ ] Safety net script cancelled (`pkill -f "sleep 300"`)
- [ ] Rules saved permanently
- [ ] Outbound connectivity verified (`curl https://google.com`)
- [ ] Cloud firewall disabled or set to allow-all
