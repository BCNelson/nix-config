terraform {
  backend "s3" {
    bucket                      = "terraform-b-nel-family"
    key                         = "state.json"
    skip_credentials_validation = true
    skip_region_validation      = true
    skip_requesting_account_id  = true
    skip_s3_checksum            = true
    skip_metadata_api_check     = true
    endpoints = {
      s3 = "https://s3.us-west-000.backblazeb2.com"
    }
    region = "us-west-004"
  }

  required_providers {
    porkbun = {
      source  = "cullenmcdermott/porkbun"
      version = "0.3.0"
    }
  }
}

variable "porkbun_api_key" {
  type = string
}

variable "porkbun_secret_key" {
  type = string
}

provider "porkbun" {
  api_key    = var.porkbun_api_key
  secret_key = var.porkbun_secret_key
}

resource "porkbun_dns_record" "CAA_nel_family" {
  domain  = "nel.family"
  type    = "CAA"
  content = "0 issue \"letsencrypt.org;validationmethods=dns-01\""
}

resource "porkbun_dns_record" "CAA_check_nel_family" {
  domain  = "nel.family"
  name    = "check"
  type    = "CAA"
  content = "0 issue \"sectigo.com\""
}

resource "porkbun_dns_record" "CAA_rss_b_nel_family" {
  domain  = "nel.family"
  name    = "rss.b"
  type    = "CAA"
  content = "0 issue \"sectigo.com\""
}

resource "porkbun_dns_record" "CAA_budget_b_nel_family" {
  domain  = "nel.family"
  name    = "budget.b"
  type    = "CAA"
  content = "0 issue \"sectigo.com\""
}

resource "porkbun_dns_record" "CAA_monitor_nel_family" {
  domain  = "nel.family"
  name    = "monitor"
  type    = "CAA"
  content = "0 issue \"sectigo.com\""
}

resource "porkbun_dns_record" "mx10_nel_family" {
  domain  = "nel.family"
  type    = "MX"
  content = "aspmx1.migadu.com"
  prio    = 10
  ttl     = 3600
}

resource "porkbun_dns_record" "mx20_nel_family" {
  domain  = "nel.family"
  type    = "MX"
  content = "aspmx2.migadu.com"
  prio    = 20
  ttl     = 3600
}

resource "porkbun_dns_record" "nel_family_txt1" {
  domain  = "nel.family"
  type    = "TXT"
  content = "hosted-email-verify=lpbdsft1"
  ttl     = 3600
}

resource "porkbun_dns_record" "nel_family_txt2" {
  domain  = "nel.family"
  type    = "TXT"
  content = "v=spf1 include:spf.migadu.com -all"
  ttl     = 3600
}

resource "porkbun_dns_record" "_dmarc_nel_family" {
  domain  = "nel.family"
  name    = "_dmarc"
  type    = "TXT"
  content = "v=DMARC1; p=quarantine;"
  ttl     = 3600
}

# h.b    300  A  76.27.3.156
resource "porkbun_dns_record" "h_b_nel_family-A" {
  domain  = "nel.family"
  name    = "h.b"
  type    = "A"
  content = "66.118.47.137"
  ttl     = 600
}

resource "porkbun_dns_record" "nixcache_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "nixcache"
  type    = "CNAME"
  content = "h.b.nel.family"
}

# atlas.b      A  100.75.145.94
resource "porkbun_dns_record" "atlas_b_nel_family-A" {
  domain  = "nel.family"
  name    = "atlas.b"
  type    = "A"
  content = "100.75.145.94"
}
# atlas.b      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:624b:915e
resource "porkbun_dns_record" "atlas_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "atlas.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:624b:915e"
}

# hypnos.b      A  100.69.108.68
resource "porkbun_dns_record" "hypnos_b_nel_family-A" {
  domain  = "nel.family"
  name    = "hypnos.b"
  type    = "A"
  content = "100.69.108.68"
}
# hypnos.b      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:6245:6c44
resource "porkbun_dns_record" "hypnos_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "hypnos.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:6245:6c44"
}
# lima.b      A  100.70.81.130
resource "porkbun_dns_record" "lima_b_nel_family-A" {
  domain  = "nel.family"
  name    = "lima.b"
  type    = "A"
  content = "100.70.81.130"
}
# lima.b      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:6246:5182
resource "porkbun_dns_record" "lima_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "lima.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:6246:5182"
}
# poseidon.b      A  100.109.254.78
resource "porkbun_dns_record" "poseidon_b_nel_family-A" {
  domain  = "nel.family"
  name    = "poseidon.b"
  type    = "A"
  content = "100.109.254.78"
}
# poseidon.b      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:626d:fe4e
resource "porkbun_dns_record" "poseidon_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "poseidon.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:626d:fe4e"
}
# public.poseidon.b      A  74.207.249.82
resource "porkbun_dns_record" "public_poseidon_b_nel_family-A" {
  domain  = "nel.family"
  name    = "public.poseidon.b"
  type    = "A"
  content = "74.207.249.82"
}
# public.poseidon.b      AAAA  2600:3c01::f03c:92ff:fe36:3ed9
resource "porkbun_dns_record" "public_poseidon_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "public.poseidon.b"
  type    = "AAAA"
  content = "2600:3c01::f03c:92ff:fe36:3ed9"
}
# whiskey.b      A  100.89.15.100
resource "porkbun_dns_record" "whiskey_b_nel_family-A" {
  domain  = "nel.family"
  name    = "whiskey.b"
  type    = "A"
  content = "100.89.15.100"
}
# whiskey.b      AAAA  fd7a:115c:a1e0::ad01:f64
resource "porkbun_dns_record" "whiskey_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "whiskey.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0::ad01:f64"
}

# public.whiskey.b      A  15.204.59.201
resource "porkbun_dns_record" "public_whiskey_b_nel_family-A" {
  domain  = "nel.family"
  name    = "public.whiskey.b"
  type    = "A"
  content = "15.204.59.201"
}
# public.whiskey.b      AAAA  2604:2dc0:202:300::b6a
resource "porkbun_dns_record" "public_whiskey_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "public.whiskey.b"
  type    = "AAAA"
  content = "2604:2dc0:202:300::b6a"
}

# romeo.b      A  100.76.49.168
resource "porkbun_dns_record" "romeo_b_nel_family-A" {
  domain  = "nel.family"
  name    = "romeo.b"
  type    = "A"
  content = "100.76.49.168"
}
# romeo.b      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:624c:31a8
resource "porkbun_dns_record" "romeo_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "romeo.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:624c:31a8"
}

resource "porkbun_dns_record" "charlie_b_nel_family-A" {
  domain  = "nel.family"
  name    = "charlie.b"
  type    = "A"
  content = "100.83.96.109"
}

resource "porkbun_dns_record" "charlie_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "charlie.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0::2f01:607e"
}

resource "porkbun_dns_record" "homeassistant_b_nel_family-A" {
  domain  = "nel.family"
  name    = "homeassistant.b"
  type    = "A"
  content = "100.66.76.95"
}

resource "porkbun_dns_record" "homeassistant_b_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "homeassistant.b"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:6242:4c5f"
}

# homeassistant.kvm      A  100.118.234.69
resource "porkbun_dns_record" "homeassistant_kvm_nel_family-A" {
  domain  = "nel.family"
  name    = "homeassistant.kvm"
  type    = "A"
  content = "100.118.234.69"
}
# homeassistant.kvm      AAAA  fd7a:115c:a1e0::4b01:ea45
resource "porkbun_dns_record" "homeassistant_kvm_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "homeassistant.kvm"
  type    = "AAAA"
  content = "fd7a:115c:a1e0::4b01:ea45"
}

# ryuu.llp      A  100.73.83.164
resource "porkbun_dns_record" "ryuu_llp_nel_family-A" {
  domain  = "nel.family"
  name    = "ryuu.llp"
  type    = "A"
  content = "100.121.151.82"
}
# ryuu.llp      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:6249:53a4
resource "porkbun_dns_record" "ryuu_llp_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "ryuu.llp"
  type    = "AAAA"
  content = "fd7a:115c:a1e0::3201:9752"
}
# vor.ck      A  100.73.83.164
resource "porkbun_dns_record" "vor_ck_nel_family-A" {
  domain  = "nel.family"
  name    = "vor.ck"
  type    = "A"
  content = "100.73.83.164"
}
# vor.ck      AAAA  fd7a:115c:a1e0:ab12:4843:cd96:6249:53a4
resource "porkbun_dns_record" "vor_ck_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "vor.ck"
  type    = "AAAA"
  content = "fd7a:115c:a1e0:ab12:4843:cd96:6249:53a4"
}

# photos      CNAME  vor.ck.nel.family.
resource "porkbun_dns_record" "photos_ck_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "photos.ck"
  type    = "CNAME"
  content = "vor.ck.nel.family"
}

# *.h.b      CNAME  h.b.nel.family.
resource "porkbun_dns_record" "wildcard_h_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "*.h.b"
  type    = "CNAME"
  content = "h.b.nel.family"
}
# *.poseidon.b      CNAME  public.poseidon.b.nel.family.
resource "porkbun_dns_record" "wildcard_poseidon_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "*.poseidon.b"
  type    = "CNAME"
  content = "public.poseidon.b.nel.family"
}
# *.romeo.b      CNAME  romeo.b.nel.family.
resource "porkbun_dns_record" "wildcard_romeo_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "*.romeo.b"
  type    = "CNAME"
  content = "romeo.b.nel.family"
}
# romeo.kvm.nel.family      A
resource "porkbun_dns_record" "romeo_kvm_nel_family-A" {
  domain  = "nel.family"
  name    = "romeo.kvm"
  type    = "A"
  content = "100.67.188.61"
}
# romeo.kvm.nel.family      AAAA
resource "porkbun_dns_record" "romeo_kvm_nel_family-AAAA" {
  domain  = "nel.family"
  name    = "romeo.kvm"
  type    = "AAAA"
  content = "fd7a:115c:a1e0::7401:bc42"
}
# audiobooks      CNAME  audiobooks.h.b.nel.family.
resource "porkbun_dns_record" "audiobooks_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "audiobooks"
  type    = "CNAME"
  content = "audiobooks.h.b.nel.family"
}
# auth      CNAME  h.b.nel.family.
resource "porkbun_dns_record" "auth_h_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "auth.h.b"
  type    = "CNAME"
  content = "h.b.nel.family"
}
# check      CNAME  esoteric-rattlesnake.pikapod.net.
resource "porkbun_dns_record" "check_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "check"
  type    = "CNAME"
  content = "esoteric-rattlesnake.pikapod.net"
}

resource "porkbun_dns_record" "monitor_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "monitor"
  type    = "CNAME"
  content = "gay-oxpecker.pikapod.net"
}

resource "porkbun_dns_record" "wildcard_arr_b_nel_family" {
  domain  = "nel.family"
  name    = "*.arr.b"
  type    = "CNAME"
  content = "h.b.nel.family"
}

# rss.b     CNAME  dainty-bull.pikapod.net
resource "porkbun_dns_record" "rss_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "rss.b"
  type    = "CNAME"
  content = "dainty-bull.pikapod.net"
}

# budget.b     CNAME  beryl-capybara.pikapod.net
resource "porkbun_dns_record" "budget_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "budget.b"
  type    = "CNAME"
  content = "beryl-capybara.pikapod.net"
}

# docs      CNAME  h.b.nel.family.
resource "porkbun_dns_record" "docs_h_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "docs.h.b"
  type    = "CNAME"
  content = "h.b.nel.family"
}
# health.b      CNAME  public.poseidon.b.nel.family.
resource "porkbun_dns_record" "health_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "health.b"
  type    = "CNAME"
  content = "public.whiskey.b.nel.family"
}
# key1._domainkey      CNAME  key1.nel.family._domainkey.migadu.com.
resource "porkbun_dns_record" "key1__domainkey_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "key1._domainkey"
  type    = "CNAME"
  content = "key1.nel.family._domainkey.migadu.com"
}
# key2._domainkey      CNAME  key2.nel.family._domainkey.migadu.com.
resource "porkbun_dns_record" "key2__domainkey_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "key2._domainkey"
  type    = "CNAME"
  content = "key2.nel.family._domainkey.migadu.com"
}
# key3._domainkey      CNAME  key3.nel.family._domainkey.migadu.com.
resource "porkbun_dns_record" "key3__domainkey_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "key3._domainkey"
  type    = "CNAME"
  content = "key3.nel.family._domainkey.migadu.com"
}
# media      CNAME  media.h.b.nel.family.
resource "porkbun_dns_record" "media_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "media"
  type    = "CNAME"
  content = "media.h.b.nel.family"
}

resource "porkbun_dns_record" "rom_nel_family" {
  domain  = "nel.family"
  name    = "rom"
  type    = "CNAME"
  content = "h.b.nel.family"
}

# recipes      CNAME  recipes.h.b.nel.family.
resource "porkbun_dns_record" "recipes_h_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "recipes"
  type    = "CNAME"
  content = "recipes.h.b.nel.family"
}
# todo      CNAME  h.b.nel.family.
resource "porkbun_dns_record" "todo_h_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "todo"
  type    = "CNAME"
  content = "h.b.nel.family"
}
# vault      CNAME  public.poseidon.b.nel.family.
resource "porkbun_dns_record" "vault_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "vault"
  type    = "CNAME"
  content = "public.whiskey.b.nel.family"
}

resource "porkbun_dns_record" "grafana_b_nel_family-CNAME" {
  domain  = "nel.family"
  name    = "grafana.b"
  type    = "CNAME"
  content = "public.whiskey.b.nel.family"
}

resource "porkbun_dns_record" "nel_to-CAA" {
  domain  = "nel.to"
  type    = "CAA"
  content = "0 issue \"letsencrypt.org;validationmethods=dns-01\""
}

resource "porkbun_dns_record" "nel_to-CNAME" {
  domain  = "nel.to"
  type    = "CNAME"
  content = "h.b.nel.family"
}

resource "porkbun_dns_record" "bcnelson_dev-CAA" {
  domain  = "bcnelson.dev"
  type    = "CAA"
  content = "0 issue \"letsencrypt.org;validationmethods=dns-01\""
}

resource "porkbun_dns_record" "git_bcnelson_dev-CNAME" {
  domain  = "bcnelson.dev"
  type    = "CNAME"
  name    = "git"
  content = "public.whiskey.b.nel.family"
}

resource "porkbun_dns_record" "idm_nel_family-CNAME" {
  domain  = "nel.family"
  type    = "CNAME"
  name    = "idm"
  content = "public.whiskey.b.nel.family"
}

resource "porkbun_dns_record" "bcnelson_dev-github-verify" {
  domain  = "bcnelson.dev"
  type    = "TXT"
  name    = "_github-pages-challenge-bcnelson"
  content = "e8a64353bcac28974a44be20b4b899"
}

resource "porkbun_dns_record" "nel_family-github-verify" {
  domain  = "nel.family"
  type    = "TXT"
  name    = "_github-pages-challenge-bcnelson"
  content = "fd1501e619123965c3b215277162ad"
}

