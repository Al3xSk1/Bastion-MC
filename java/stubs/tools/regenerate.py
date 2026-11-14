#!/usr/bin/env python3
"""Regenerates java/stubs/minecraft/ from Mojang's and Fabric's published files.

    regenerate.py <download dir> [--check]

1. fetch.py downloads every version named in families.txt into <download dir> (hashes
   checked against what Mojang and Fabric publish) and its SHA-256 sums are compared with
   java/stubs/minecraft/SOURCES.txt (written there when absent or different, without --check);
2. genstubs.py writes the stubs of every version into <download dir>/stubs/<version>;
3. versions are grouped by their SIGNATURES.txt: each group must be exactly one family of
   families.txt, else this stops and says which versions differ;
4. the representative's stubs become java/stubs/minecraft/<family>/ (replaced whole).

With --check nothing in the repository is changed; it fails when the result would differ.
"""
import filecmp, hashlib, os, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
STUBS = os.path.dirname(HERE)
OUT = os.path.join(STUBS, 'minecraft')


def families():
    out = []
    for line in open(os.path.join(HERE, 'families.txt')):
        line = line.split('#', 1)[0].split()
        if line:
            out.append((line[0], line[2], line[3:]))
    return out


def run(args):
    return subprocess.run([sys.executable, '-I', '-B'] + args, check=True, capture_output=True, text=True).stdout


def tree_equal(a, b):
    cmp = filecmp.dircmp(a, b)
    if cmp.left_only or cmp.right_only or cmp.diff_files or cmp.funny_files:
        return False
    return all(tree_equal(os.path.join(a, d), os.path.join(b, d)) for d in cmp.common_dirs)


def main():
    dl = sys.argv[1]
    check = '--check' in sys.argv[2:]
    fams = families()
    versions = [v for _, _, vs in fams for v in vs]
    sums = run([os.path.join(HERE, 'fetch.py'), dl] + versions)
    sources = os.path.join(OUT, 'SOURCES.txt')
    header = ('# SHA-256 of every file java/stubs/tools/fetch.py downloaded to generate the stubs here\n'
              '# (each also checked against the hash Mojang or Fabric publishes for it).\n')
    if check:
        if open(sources).read() != header + sums:
            sys.exit('the downloaded files differ from SOURCES.txt')
    signatures = {}
    for v in versions:
        d = os.path.join(dl, v)
        out = os.path.join(dl, 'stubs', v)
        shutil.rmtree(out, ignore_errors=True)
        args = [os.path.join(HERE, 'genstubs.py'), '--jar', os.path.join(d, 'client.jar'), '--needed',
                os.path.join(HERE, 'needed.txt'), '--out', out, '--version', v,
                '--names', os.path.join(out, 'NAMES.txt')]
        if os.path.exists(os.path.join(d, 'client.txt')):
            args += ['--mappings', os.path.join(d, 'client.txt'), '--intermediary',
                     os.path.join(d, 'intermediary-v2.jar')]
        if os.path.exists(os.path.join(d, 'joml.jar')):
            args += ['--lib', os.path.join(d, 'joml.jar')]
        print(run(args).strip())
        signatures[v] = hashlib.sha256(open(os.path.join(out, 'SIGNATURES.txt'), 'rb').read()).hexdigest()
    problems = []
    for fam, rep, vs in fams:
        if len({signatures[v] for v in vs}) != 1:
            problems.append('%s: its versions differ: %s' % (fam, ', '.join('%s=%s' % (v, signatures[v][:12]) for v in vs)))
    for i, (fam, _, vs) in enumerate(fams):
        for other, _, ws in fams[i + 1:]:
            if signatures[vs[0]] == signatures[ws[0]]:
                problems.append('%s and %s have the same signatures: one family' % (fam, other))
    if problems:
        sys.exit('\n'.join(problems))
    changed = False
    for fam, rep, vs in fams:
        src = os.path.join(dl, 'stubs', rep)
        dst = os.path.join(OUT, fam)
        if os.path.isdir(dst) and tree_equal(src, dst):
            continue
        changed = True
        if check:
            print('%s would change' % dst)
            continue
        shutil.rmtree(dst, ignore_errors=True)
        shutil.copytree(src, dst)
    if check and changed:
        sys.exit('the stubs in the repository are not what the published files generate')
    if not check:
        with open(sources, 'w') as f:
            f.write(header + sums)
    print('families: ' + ', '.join('%s (%s)' % (f, ' '.join(vs)) for f, _, vs in fams))


if __name__ == '__main__':
    main()
