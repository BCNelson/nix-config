package hostprobe

import "testing"

// Output captured from romeo-2 (trimmed).
const romeoOut = `state.BUILT_COMMIT=cc7d7e83f610739d5857b59d48d45e2bf8f3b601
state.ATTEMPT_COMMIT=
state.ATTEMPT_COUNT=0
svc.LoadState=loaded
svc.ActiveState=inactive
svc.SubState=dead
svc.InvocationID=40800058bcad4b6c95392e67cabb8ccc
svc.Result=success
svc.ExecMainStartTimestamp=@1792299869
svc.ExecMainExitTimestamp=@1792299959
svc.ExecMainStatus=0
env.CONFIG_PATH=/config
env.MAX_RETRIES=3
env.REBOOT=true
ntfy=active
timer.json=[{"next":1792303469151377,"left":1792303469151377,"last":1792291552121493,"passed":1354752443619,"unit":"auto-update.timer","activates":"auto-update.service"}]
git.branch=auto-update
git.head=cc7d7e83f610739d5857b59d48d45e2bf8f3b601
git.crypt_unlocked=1
sys.current=/nix/store/0p942lr6sx1kdws6z00vbj6fcmb6q2hr-nixos-system-romeo-2-26.11pre-git
sys.profile=/nix/store/0p942lr6sx1kdws6z00vbj6fcmb6q2hr-nixos-system-romeo-2-26.11pre-git
sys.booted=/nix/store/ydivmxb87dyv8f5asp0lj1lsds5hqqxh-nixos-system-romeo-2-26.11pre-git
sys.cur.initrd=/nix/store/i1-initrd
sys.boot.initrd=/nix/store/i1-initrd
sys.cur.kernel=/nix/store/k1/bzImage
sys.boot.kernel=/nix/store/k1/bzImage
sys.cur.kernel-modules=/nix/store/m1
sys.boot.kernel-modules=/nix/store/m1
sys.bootid=cb48bf42-ac4a-48e8-b2cf-0ce621c76cc4
sys.state=running
sys.now=1792300000
probe.done=1
`

func TestParse(t *testing.T) {
	s := toState("romeo-2", parseKV(romeoOut))
	if !s.Enabled || s.Branch != "auto-update" || s.BuiltCommit[:7] != "cc7d7e8" || s.MaxRetries != 3 || !s.Reboot || !s.Ntfy || !s.Unlocked {
		t.Fatalf("%+v", s)
	}
	if s.Running() || s.SwitchPending() || s.NeedsReboot() {
		t.Fatalf("idle host misread: %+v", s)
	}
	if s.NextTimer.Unix() != 1792303469 || s.StartedAt.Unix() != 1792299869 {
		t.Fatalf("times: %v %v", s.NextTimer, s.StartedAt)
	}
	kv := parseKV(romeoOut)
	kv["sys.cur.kernel"] = "/nix/store/k2/bzImage"
	if !toState("romeo-2", kv).NeedsReboot() {
		t.Fatal("kernel change not detected")
	}
}
