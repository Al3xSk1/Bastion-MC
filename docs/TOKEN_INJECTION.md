# Secure token injection

How the Minecraft access token gets from the launcher into the game without ever appearing
in `/proc/<pid>/cmdline`, the environment, a file, or any other place another process can
look — and exactly where that guarantee ends.

## What travels on this path today

The mechanism below was built to carry the access token, and still does when the token has
to be in the game. By default it no longer has to be: in `filtered` network mode the
**credential broker** is on, the launcher keeps the real token, and what is written into
the launch frame as `--accessToken` is a **placeholder** — `bastion-placeholder-` and 32
random hexadecimal digits, new at every launch, worth nothing anywhere else. The launcher
terminates the game's HTTPS to Mojang's API (a per-session certificate authority that
exists only in the sandbox's trust store) and puts the real token on the requests that are
switched on; see `THREAT_MODEL.md`, row J13.

| Launch | What the frame carries as the access token | Where the real token is |
|---|---|---|
| `filtered` network, broker on (default) | the placeholder | in the launcher, in secret memory, for the length of the session; never in the sandbox |
| `--demo` | the constant `0` | there is none |
| `--burner`, `network.mode = "none"` | the constant `0` or the placeholder | not minted (burner), or in no process once the frame is written |
| `open` network, or `network.broker.enabled = false` | **the real token** | in the JVM; the launcher forgets it as described below. The launch log and the front end say so. |

Everything that follows — no command line, no environment, a socketpair on fd 0 — applies
to all four rows: a placeholder that leaked would still be an unnecessary gift, and the
last row is why the path exists.

## The problem

The vanilla client takes its session as program arguments:

```
java … net.minecraft.client.main.Main --username Steve --uuid … --accessToken eyJhbGci…
```

Everything after `java` is world-readable for the life of the process: `ps aux`,
`/proc/<pid>/cmdline`, process accounting, audit logs, crash reporters. The JVM also copies
it into the `sun.java.command` system property, the `hsperfdata` file and `hs_err` crash
logs. An environment variable is no better (`/proc/<pid>/environ` is readable by every
process with your UID and is inherited by every child).

## The mechanism

```
 launcher                                    sandbox
 ────────                                    ───────
 access token in mlock'd Secret
        │
 build_frame()  ──────────────►  [ one mlock'd buffer, exact size ]
        │                          "BSTN" ver count  len main-class  len arg … len arg
        │ write(), shutdown(SHUT_WR)
        ▼
   socketpair(AF_UNIX) ════════ fd 0 ═══════►  bwrap ─► init ─► java -cp … BastionBootstrap
        │                                                         │ (no program arguments)
 frame.wipe(); token.wipe()                                       │ read frame from fd 0
 launcher now holds no credential                                 │ zero the byte buffers
                                                                  ▼
                                               Main.main(String[]{…, "--accessToken", token})
```

1. **The JVM is started with no program arguments.** Its command line is the JVM options,
   the class path and the literal class name `BastionBootstrap`
   ([`java/BastionBootstrap.java`](../java/BastionBootstrap.java), 118 lines, Java 8
   bytecode, compiled from source by `build.rs` — no binary blob in the repository).
2. **fd 0 of the JVM is one end of an `AF_UNIX` `SOCK_STREAM` socketpair** created by the
   launcher ([`sandbox::spawn`](../src/sandbox/mod.rs)). bubblewrap and the init stage pass
   it through untouched; every other descriptor is closed before the JVM is executed.
3. **The launcher writes a single frame** and half-closes the socket
   ([`mojang::args::build_frame`](../src/mojang/args.rs)):

   ```
   u32 0x4253544E ("BSTN")   u32 version = 1   u32 count
   count × { u32 byteLength, UTF-8 bytes }      // [0] = main class, [1..] = its arguments
   ```

   The frame is assembled directly inside one `mlock`ed, `MADV_DONTDUMP`,
   `MADV_WIPEONFORK` buffer of exactly the final size. The token is expanded from the
   version file's `${auth_access_token}` template *into that buffer*; it never passes
   through a `String`. Templates that would put a secret anywhere else are refused:
   `render_plain`, which produces the JVM's command line, returns an error if a template
   mentions a secret variable — so a malformed or hostile version file cannot move the token
   onto the command line.
4. **The bootstrap** reads the frame with an *unbuffered* `FileInputStream` on
   `FileDescriptor.in` (the default `System.in` is buffered and would keep a second copy),
   zeroes each byte buffer after decoding, replaces `System.in` with an empty stream,
   insists on EOF after the frame, then calls the real `main(String[])` reflectively.
   Reflective arguments do not appear in `sun.java.command`, `hsperfdata` or `hs_err`.
5. **The launcher wipes** the frame and the token (`explicit_bzero`, then `munmap`) as soon
   as the write returns, and has already written the rotated refresh token back to its
   store and wiped that too. While the game runs, the launcher holds no credential.

### Why a socketpair and not a pipe, a memfd or a file

Measured on the target system: a second process with the same UID opened
`/proc/<victim>/fd/0` while the victim held an unread token.

| channel | result |
|---|---|
| pipe | token read by the other process |
| memfd (the "anonymous file descriptor" approach) | token read by the other process |
| `AF_UNIX` socketpair | `ENXIO` — a socket cannot be re-opened through procfs |

A socketpair also needs no name anywhere in the filesystem, cannot be sealed-and-kept, and
disappears with its last descriptor.

### Token hygiene on the launcher side

| Concern | Measure |
|---|---|
| Swap | every token lives in its own locked mapping — `memfd_secret` pages where the kernel offers them, an `mlock`ed anonymous mapping otherwise; allocation fails rather than fall back to pageable memory |
| Reads through the kernel (`/proc/<pid>/mem`, `process_vm_readv`, ptrace, `/proc/kcore`) | with `memfd_secret` the pages are not in the kernel's direct map: such reads fail even for a process that is allowed to attach. Not a defence against code in the kernel, or against cold-boot and DMA attacks |
| Core dumps | `PR_SET_DUMPABLE=0`, `MADV_DONTDUMP`, `RLIMIT_CORE=1` (not 0: with a piped `core_pattern` the kernel ignores 0) |
| fork/exec window | `MADV_WIPEONFORK` (a forked child sees zero pages) or, for `memfd_secret`, `MADV_DONTFORK` (the child does not have the mapping at all) |
| Copies in buffers we do not own (rustls records, hyper buffers, serde scratch, `Vec` growth) | a global allocator that `explicit_bzero`s **every** block on free and implements `realloc` as alloc-copy-wipe-free |
| JSON parsing | token fields deserialise straight into locked memory (`impl Deserialize for Secret`); request bodies carrying tokens are built as bytes, never formatted into a `String` |
| Debug output | `Secret`'s `Debug` prints a length only; `Authorization` headers are marked sensitive |
| Peeking at the launcher | non-dumpable + Yama: `/proc/<pid>/mem`, `process_vm_readv` and ptrace are refused for other same-UID processes, including the parent shell |
| Lifetime | Microsoft token → dropped after the Xbox call; Xbox token → after XSTS; XSTS → after Minecraft login; Minecraft token → with the broker: held until the game exits, then wiped; without: wiped once the frame is written |
| While the game runs | the launcher confines itself, on all threads and before it reads anything the game sends: Landlock (no program execution, no file access outside the instance's `persist/` directory and the cache, TCP only to the ports it relays to) and a system-call filter (no kernel keyring, where the refresh token rests by default) |

## What other processes see

From the real Minecraft 26.3 client running in the sandbox on the development machine:

```
$ tr '\0' ' ' < /proc/<jvm>/cmdline | tail -c 120
/tmp -Duser.home=/game -Djdk.serialFilter=!* -XX:-CreateCoredumpOnCrash -XX:-EnableDynamicAgentLoading BastionBootstrap

$ tr '\0' ' ' < /proc/<jvm>/environ
PATH=/usr/bin HOME=/game USER=player LOGNAME=player TMPDIR=/tmp XDG_RUNTIME_DIR=/run/user/1000
LANG=en_US.UTF-8 DISPLAY=:0 XDG_SESSION_TYPE=x11 XAUTHORITY=/run/user/1000/Xauthority
PULSE_SERVER=unix:/run/user/1000/pulse/native

$ tr '\0' ' ' < /proc/<bwrap>/cmdline
/usr/bin/bwrap --args 90 -- /bastion/init __sandbox-init 91
```

No token, no player name, no UUID, no home directory, no instance name, no real user name.
The launcher's own log records the game arguments with the token shown as `<redacted>`.

## Where the guarantee ends

Stating these plainly is part of the design:

* **Inside the JVM the token is an ordinary `java.lang.String`.** Minecraft keeps it for the
  whole session because it needs it to join servers; Java strings are immutable and cannot
  be zeroized. "Zeroize after the handshake" is achievable for the launcher and for the
  bootstrap's transport buffers, not for the game's own copy.
* **Any code in the JVM can read it** (`Minecraft.getInstance().getUser().getAccessToken()`).
  A malicious mod is contained by the sandbox and the egress policy, not by hiding the
  token from it.
* **Other software running as you can read the JVM's memory**, even with
  `kernel.yama.ptrace_scope=1` and even if the JVM were non-dumpable, because the JVM lives
  in a user namespace your UID owns. This was reproduced (`process_vm_readv` succeeded).
  Only `ptrace_scope=3` or a dedicated UNIX account prevents it.
* The structural fix for all three is the **credential broker**: the game gets a
  placeholder, and the launcher swaps in the real token on the few Mojang API calls that
  need it. It is built and on by default in `filtered` mode (the table at the top). The
  three points above describe a launch *without* it. With it, what remains is that code in
  the game can make the requests the broker is willing to authenticate — one server join
  per open game connection, and reads of the account's privileges and block list — but
  cannot take the credential out of the sandbox, because it was never in it.

## Checking it yourself

```sh
cargo test --test jvm      # real JVM in the sandbox; asserts the token is in argv[] of
                           # main() but not in cmdline / environ / sun.java.command
cargo test --test sandbox launch_channel_is_not_observable

# against a running game
pid=$(pgrep -f ' BastionBootstrap$')
tr '\0' '\n' < /proc/$pid/cmdline | grep -c -e --accessToken      # 0
ls -l /proc/$pid/fd/0                                             # socket:[…]
cat /proc/$pid/fd/0                                               # No such device or address
```
