#!/usr/bin/env python3
"""Downloads what genstubs.py reads, each file checked against the hash its publisher gives:

    fetch.py <dir> <version>...

* Mojang's version manifest, then each version's JSON (SHA-1 from the manifest);
* the client jar and, for obfuscated versions, Mojang's client mappings (SHA-1 from the
  version JSON); the JOML library jar (SHA-1 from the version JSON);
* Fabric's intermediary for obfuscated versions (SHA-512 from Fabric's maven, next to it).

Then prints the SHA-256 of every file, which java/stubs/minecraft/SOURCES.txt records: a
repetition must produce the same digests (Mojang's and Fabric's files do not change once
published), and genstubs.py the same stubs.
"""
import hashlib, json, os, sys, urllib.request

MANIFEST = 'https://piston-meta.mojang.com/mc/game/version_manifest_v2.json'


def get(url):
    req = urllib.request.Request(url, headers={'User-Agent': 'curl'})
    return urllib.request.urlopen(req, timeout=120).read()


def fetch(url, path, algo, want):
    if os.path.exists(path) and hashlib.new(algo, open(path, 'rb').read()).hexdigest() == want:
        return
    data = get(url)
    got = hashlib.new(algo, data).hexdigest()
    if got != want:
        sys.exit('%s: %s is %s, expected %s' % (url, algo, got, want))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'wb') as f:
        f.write(data)


def main():
    root, versions = sys.argv[1], sys.argv[2:]
    os.makedirs(root, exist_ok=True)
    manifest = json.loads(get(MANIFEST))
    index = {v['id']: v for v in manifest['versions']}
    for v in versions:
        meta = index[v]
        d = os.path.join(root, v)
        fetch(meta['url'], os.path.join(d, v + '.json'), 'sha1', meta['sha1'])
        j = json.load(open(os.path.join(d, v + '.json')))
        c = j['downloads']['client']
        fetch(c['url'], os.path.join(d, 'client.jar'), 'sha1', c['sha1'])
        cm = j['downloads'].get('client_mappings')
        if cm:
            fetch(cm['url'], os.path.join(d, 'client.txt'), 'sha1', cm['sha1'])
            base = 'https://maven.fabricmc.net/net/fabricmc/intermediary/%s/intermediary-%s-v2.jar' % (v, v)
            want = get(base + '.sha512').decode().split()[0].strip()
            fetch(base, os.path.join(d, 'intermediary-v2.jar'), 'sha512', want)
        for lib in j['libraries']:
            if lib['name'].startswith('org.joml:joml:') and 'artifact' in lib.get('downloads', {}):
                a = lib['downloads']['artifact']
                fetch(a['url'], os.path.join(d, 'joml.jar'), 'sha1', a['sha1'])
        for name in sorted(os.listdir(d)):
            data = open(os.path.join(d, name), 'rb').read()
            print('%s  %s/%s' % (hashlib.sha256(data).hexdigest(), v, name))


if __name__ == '__main__':
    main()
