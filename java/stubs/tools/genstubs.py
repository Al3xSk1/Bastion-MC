#!/usr/bin/env python3
"""Compile-only stand-ins ("stubs") of the Minecraft classes the companion mod's adapters
call, for one Minecraft version, in the names the game has at run time.

    genstubs.py --jar <client.jar> [--mappings client.txt --intermediary intermediary-v2.jar]
                [--lib <library.jar>]... --needed needed.txt --out <dir> [--names NAMES.txt]

* Obfuscated versions (up to 1.21.11) run under Fabric's intermediary names: give Mojang's
  client mappings and Fabric's intermediary for the version. Members are looked up by
  Mojang's names (needed.txt) and written out under intermediary names, with Mojang's name
  in a comment.
* Unobfuscated versions (26.1 on) need only the jar.

What a stub holds: the class's kind (class, interface, enum), its supertypes, and the members
named in needed.txt — names, descriptors, static-ness and visibility, read from the jar's
class files. No method body, no constant value (fields are written without `final`, so that
javac never inlines a value that differs between versions), no generic signature. Types that
a member mentions, and the supertypes of every stub, become empty stubs. Each stub class can
be constructed through a constructor taking `stubs.Marker`, which no real class has: it only
exists so that stub subclasses compile; nothing outside the stubs can name the marker.

The stubs are never shipped: the build compiles them into a directory of their own that is
on the class path of the adapter's compilation only.
"""
import argparse
import collections
import os
import sys
import zipfile

sys.dont_write_bytecode = True  # nothing written next to the tools
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import classfile  # noqa: E402
from maps import Mojang, Tiny, jtype_to_desc, map_desc, split_desc  # noqa: E402

ACC = classfile


def parse_needed(path):
    """[(alternative Mojang names, {member name: [mojang descriptors] or None}, {hook targets})]"""
    out = []
    cur = None
    for raw in open(path, encoding='utf-8'):
        line = raw.split('#', 1)[0].rstrip()
        if not line.strip():
            continue
        if not raw[0].isspace():
            names = [n.strip().replace('.', '/') for n in line.split('|')]
            cur = (names, collections.OrderedDict(), set())
            out.append(cur)
        else:
            parts = line.split()
            name = parts[0]
            desc = parts[1] if len(parts) > 1 else None
            members = cur[1]
            if name.startswith('@'):
                cur[2].add(name[1:])
                continue
            if desc is None:
                members[name] = None
            elif members.get(name, []) is not None:
                members.setdefault(name, []).append(desc)
    return out


class Source:
    """The classes of the client jar (and libraries) in runtime names."""

    def __init__(self, jar, mappings, intermediary, libs):
        self.jars = [zipfile.ZipFile(jar)] + [zipfile.ZipFile(l) for l in libs]
        self.entries = {}
        for z in self.jars:
            for e in z.namelist():
                if e.endswith('.class'):
                    self.entries.setdefault(e[:-6], (z, e))
        self.parsed = {}
        self.obfuscated = mappings is not None
        if self.obfuscated:
            self.m = Mojang(mappings)
            self.t = Tiny(intermediary)
            self.o2i = self.t.obf_to_int
            self.i2o = {v: k for k, v in self.o2i.items()}
            self.named = {}
            for named, c in self.m.classes.items():
                d = {}
                for name, typ, obf in c['fields']:
                    d[('f', obf, jtype_to_desc(typ, self.m.named_to_obf))] = name
                for name, params, ret, obf in c['methods']:
                    od = '(' + ''.join(jtype_to_desc(p, self.m.named_to_obf) for p in params) + ')' + \
                        jtype_to_desc(ret, self.m.named_to_obf)
                    d[('m', obf, od)] = name
                self.named[c['obf']] = d

    def raw(self, obf):
        if obf not in self.parsed:
            hit = self.entries.get(obf)
            self.parsed[obf] = classfile.parse(hit[0].read(hit[1])) if hit else None
        return self.parsed[obf]

    # names --------------------------------------------------------------------------
    def to_runtime(self, obf):
        return self.o2i.get(obf, obf) if self.obfuscated else obf

    def to_obf(self, runtime):
        return self.i2o.get(runtime, runtime) if self.obfuscated else runtime

    def mojang_of(self, runtime):
        if not self.obfuscated:
            return runtime
        return self.m.obf_to_named.get(self.to_obf(runtime), runtime)

    def runtime_of_mojang(self, mojang):
        if not self.obfuscated:
            return mojang if mojang in self.entries else None
        obf = self.m.named_to_obf.get(mojang)
        if obf is None:
            return mojang if mojang in self.entries and mojang not in self.m.obf_to_named else None
        return self.to_runtime(obf)

    def runtime_desc(self, obf_desc):
        return map_desc(obf_desc, self.o2i) if self.obfuscated else obf_desc

    def mojang_desc(self, obf_desc):
        return map_desc(obf_desc, self.m.obf_to_named) if self.obfuscated else obf_desc

    def _int_member(self, obf_owner, kind, name, desc, seen):
        if obf_owner in seen:
            return None
        seen.add(obf_owner)
        tc = self.t.classes.get(obf_owner)
        if tc:
            table = tc['fields'] if kind == 'f' else tc['methods']
            if (name, desc) in table:
                return table[(name, desc)]
        if kind == 'f':
            return None
        info = self.raw(obf_owner)
        if not info:
            return None
        for sup in [info.super] + info.interfaces:
            if sup and sup in self.entries:
                found = self._int_member(sup, kind, name, desc, seen)
                if found:
                    return found
        return None

    def members(self, runtime):
        """(info, [(kind, access, runtime name, runtime desc, mojang name, mojang desc)])"""
        obf = self.to_obf(runtime)
        info = self.raw(obf)
        if info is None:
            return None, []
        out = []
        for kind, items in (('f', info.fields), ('m', info.methods)):
            for access, name, desc in items:
                if not self.obfuscated:
                    out.append((kind, access, name, desc, name, desc))
                    continue
                mojang = self.named.get(obf, {}).get((kind, name, desc), name)
                if name in ('<init>', '<clinit>'):
                    rname = name
                else:
                    rname = self._int_member(obf, kind, name, desc, set()) or name
                out.append((kind, access, rname, self.runtime_desc(desc), mojang, self.mojang_desc(desc)))
        return info, out


def java_type(desc, i=0):
    """Descriptor at i -> (java source type, next index)."""
    dims = 0
    while desc[i] == '[':
        dims += 1
        i += 1
    c = desc[i]
    prims = {'I': 'int', 'F': 'float', 'D': 'double', 'J': 'long', 'Z': 'boolean', 'B': 'byte', 'C': 'char',
             'S': 'short', 'V': 'void'}
    if c in prims:
        t, i = prims[c], i + 1
    else:
        end = desc.index(';', i)
        t, i = source_name(desc[i + 1:end]), end + 1
    return t + '[]' * dims, i


def source_name(internal):
    return internal.replace('/', '.').replace('$', '.')


def desc_classes(desc):
    out = []
    i = 0
    while i < len(desc):
        if desc[i] == 'L':
            end = desc.index(';', i)
            out.append(desc[i + 1:end])
            i = end + 1
        else:
            i += 1
    return out


def default_value(t):
    return {'int': '0', 'float': '0f', 'double': '0d', 'long': '0L', 'boolean': 'false', 'byte': '(byte) 0',
            'char': "'\\0'", 'short': '(short) 0'}.get(t, '(%s) null' % t)


JDK_PREFIXES = ('java/', 'javax/', 'jdk/', 'sun/')


def generate(src, needed, out_dir, names_path, version):
    wanted = collections.OrderedDict()   # runtime class -> {member key: True} or None (empty stub)
    mojang_names = {}
    missing = []
    targets = collections.defaultdict(set)   # runtime class -> Mojang names of injection targets
    for alternatives, members, hooks in needed:
        runtime = None
        for alt in alternatives:
            runtime = src.runtime_of_mojang(alt)
            if runtime:
                break
        if runtime is None:
            if not hooks:
                missing.append(alternatives[0])
            continue
        if hooks:
            targets[runtime] |= hooks
        if not members and hooks:
            continue
        wanted.setdefault(runtime, {})
        if wanted[runtime] is None:
            wanted[runtime] = {}
        wanted[runtime].update({name: descs for name, descs in members.items()})
        mojang_names[runtime] = src.mojang_of(runtime)

    whitelisted = set(wanted.keys())

    def ancestors(runtime):
        """Every supertype of `runtime` (runtime names), nearest first."""
        out, queue = [], [runtime]
        while queue:
            info = src.raw(src.to_obf(queue.pop(0)))
            if info is None:
                continue
            for sup in [info.super] + info.interfaces:
                if sup:
                    r = src.to_runtime(sup)
                    if r not in out:
                        out.append(r)
                        queue.append(r)
        return out

    def flattened_supertypes(runtime):
        """(superclass, interfaces) among the whitelisted classes: the nearest whitelisted
        superclass, and every whitelisted interface the class has, directly or not."""
        sup = None
        info = src.raw(src.to_obf(runtime))
        cur = info
        while cur is not None and cur.super:
            r = src.to_runtime(cur.super)
            if r in whitelisted:
                sup = r
                break
            cur = src.raw(src.to_obf(r))
        interfaces = []
        for r in ancestors(runtime):
            ri = src.raw(src.to_obf(r))
            if r in whitelisted and ri is not None and ri.access & ACC.ACC_INTERFACE and r != sup:
                interfaces.append(r)
        return sup, interfaces

    # Which members each stub has, then the closure of the types they mention.
    stubs = collections.OrderedDict()   # runtime -> (info, [member tuples], superclass, interfaces)
    pending = list(wanted.keys())
    while pending:
        runtime = pending.pop(0)
        if runtime in stubs or runtime.startswith(JDK_PREFIXES):
            continue
        info, members = src.members(runtime)
        if info is None:
            stubs[runtime] = (None, [], None, [])
            continue
        chosen = []
        spec = wanted.get(runtime) or {}
        seen = set()
        for m in members:
            kind, access, rname, rdesc, mname, mdesc = m
            if access & (ACC.ACC_PRIVATE | ACC.ACC_SYNTHETIC) or mname == '<clinit>':
                continue
            if not (access & (ACC.ACC_PUBLIC | ACC.ACC_PROTECTED)):
                continue
            if mname not in spec:
                continue
            if spec[mname] is not None and mdesc not in spec[mname]:
                continue
            key = (kind, rname, rdesc if kind == 'f' else rdesc[:rdesc.index(')') + 1])
            if key in seen:
                continue
            seen.add(key)
            chosen.append(m)
        sup, interfaces = flattened_supertypes(runtime) if runtime in whitelisted else (None, [])
        stubs[runtime] = (info, chosen, sup, interfaces)
        refs = [r for r in [sup] + interfaces if r]
        for m in chosen:
            refs.extend(desc_classes(m[3]))
        if '$' in runtime:
            refs.append(runtime[:runtime.rindex('$')])
        for r in refs:
            if r not in stubs and not r.startswith(JDK_PREFIXES):
                pending.append(r)
        for r in refs:
            mojang_names.setdefault(r, src.mojang_of(r))

    # A listed class with nothing chosen from it, that nothing else mentions, is left out:
    # whether it exists in a version does not matter to code that does not use it.
    mentioned = set()
    for runtime, (info, members, sup, interfaces) in stubs.items():
        mentioned.update(r for r in [sup] + interfaces if r)
        for m in members:
            mentioned.update(desc_classes(m[3]))
        if '$' in runtime:
            mentioned.add(runtime[:runtime.rindex('$')])
    for runtime in list(stubs):
        if not stubs[runtime][1] and runtime not in mentioned:
            del stubs[runtime]

    # Group nested classes under their top-level class.
    tops = collections.OrderedDict()
    for runtime in sorted(stubs):
        top = runtime.split('$')[0]
        tops.setdefault(top, [])
        if runtime != top:
            tops[top].append(runtime)
        if top not in stubs:
            stubs[top] = (src.members(top)[0], [], None, [])

    def ctor_call(runtime):
        """`super(...)` for a stub constructor of `runtime`'s stub class."""
        sup = stubs[runtime][2]
        if sup and sup in stubs and stubs[sup][0] is not None:
            return 'super((stubs.Marker) null);'
        return 'super();'

    def emit_class(runtime, indent, lines):
        info, members, sup, interfaces = stubs[runtime]
        simple = runtime.split('/')[-1].split('$')[-1]
        pad = '    ' * indent
        if info is None:
            lines.append('%s/** Not in this version\'s jar: an empty stand-in. */' % pad)
            lines.append('%spublic %sclass %s {' % (pad, 'static ' if indent else '', simple))
            lines.append('%s}' % pad)
            return
        is_interface = bool(info.access & ACC.ACC_INTERFACE)
        is_enum = bool(info.access & ACC.ACC_ENUM) and info.super == 'java/lang/Enum'
        is_annotation = bool(info.access & 0x2000)
        mods = ['public']
        if indent:
            mods.append('static')
        kind = 'class'
        if is_annotation:
            kind = '@interface'
        elif is_interface:
            kind = 'interface'
        elif is_enum:
            kind = 'enum'
        else:
            if info.access & ACC.ACC_ABSTRACT:
                mods.append('abstract')
        header = '%s%s %s %s' % (pad, ' '.join(mods), kind, simple)
        mojang = mojang_names.get(runtime, runtime)
        if kind == 'class' and sup:
            header += ' extends ' + source_name(sup)
        if interfaces and kind in ('class', 'enum'):
            header += ' implements ' + ', '.join(source_name(i) for i in interfaces)
        if interfaces and kind == 'interface':
            header += ' extends ' + ', '.join(source_name(i) for i in interfaces)
        if mojang != runtime:
            lines.append('%s/** %s */' % (pad, mojang.replace('/', '.')))
        lines.append(header + ' {')
        inner = pad + '    '
        if kind == 'enum':
            constants = [m for m in members if m[0] == 'f' and m[1] & ACC.ACC_ENUM]
            lines.append(inner + ', '.join(m[2] for m in constants) + ';')
            members = [m for m in members if not (m[0] == 'f' and m[1] & ACC.ACC_ENUM)]
        if kind == 'class':
            lines.append('%s/** Stub-only. */' % inner)
            lines.append('%sprotected %s(stubs.Marker marker) {' % (inner, simple))
            lines.append('%s    %s' % (inner, ctor_call(runtime)))
            lines.append('%s}' % inner)
        for m in members:
            kind_m, access, rname, rdesc, mname, mdesc = m
            vis = 'public' if access & ACC.ACC_PUBLIC else 'protected'
            static = access & ACC.ACC_STATIC
            comment = '' if (mname == rname and mdesc == rdesc) else ' // %s %s' % (mname, mdesc)
            if kind_m == 'f':
                t, _ = java_type(rdesc)
                if kind == 'interface':
                    lines.append('%s%s %s = %s;%s' % (inner, t, rname, default_value(t), comment))
                else:
                    lines.append('%s%s %s%s %s;%s' % (inner, vis, 'static ' if static else '', t, rname, comment))
                continue
            args, ret = split_desc(rdesc)
            params = []
            for n, a in enumerate(args):
                t, _ = java_type(a)
                if n == len(args) - 1 and access & ACC.ACC_VARARGS and t.endswith('[]'):
                    t = t[:-2] + '...'
                params.append('%s p%d' % (t, n))
            if rname == '<init>':
                if kind != 'class':
                    continue
                lines.append('%s%s %s(%s) {%s' % (inner, vis, simple, ', '.join(params), comment))
                lines.append('%s    this((stubs.Marker) null);' % inner)
                lines.append('%s    throw new AssertionError();' % inner)
                lines.append('%s}' % inner)
                continue
            rt, _ = java_type(ret)
            abstract = access & ACC.ACC_ABSTRACT
            if kind == 'interface':
                if static:
                    lines.append('%sstatic %s %s(%s) {%s' % (inner, rt, rname, ', '.join(params), comment))
                    lines.append('%s    throw new AssertionError();' % inner)
                    lines.append('%s}' % inner)
                elif abstract:
                    lines.append('%s%s %s(%s);%s' % (inner, rt, rname, ', '.join(params), comment))
                else:
                    lines.append('%sdefault %s %s(%s) {%s' % (inner, rt, rname, ', '.join(params), comment))
                    lines.append('%s    throw new AssertionError();' % inner)
                    lines.append('%s}' % inner)
                continue
            if kind == '@interface':
                continue
            if abstract:
                lines.append('%s%s abstract %s %s(%s);%s' % (inner, vis, rt, rname, ', '.join(params), comment))
            else:
                lines.append('%s%s %s%s %s(%s) {%s' % (inner, vis, 'static ' if static else '', rt, rname,
                                                      ', '.join(params), comment))
                lines.append('%s    throw new AssertionError();' % inner)
                lines.append('%s}' % inner)
        for nested in sorted(n for n in stubs if n.startswith(runtime + '$') and n.count('$') == runtime.count('$') + 1):
            emit_class(nested, indent + 1, lines)
        lines.append('%s}' % pad)

    written = []
    for top in tops:
        if top.startswith(JDK_PREFIXES):
            continue
        lines = ['// Generated by java/stubs/tools/genstubs.py from the Minecraft %s client jar: names and' % version,
                 '// signatures only, no code. Compile-only; never shipped. Do not edit: regenerate.']
        pkg = '/'.join(top.split('/')[:-1])
        if pkg:
            lines.append('package %s;' % source_name(pkg))
        lines.append('')
        emit_class(top, 0, lines)
        path = os.path.join(out_dir, top + '.java')
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, 'w', encoding='utf-8') as f:
            f.write('\n'.join(lines) + '\n')
        written.append(top)
    marker = os.path.join(out_dir, 'stubs', 'Marker.java')
    os.makedirs(os.path.dirname(marker), exist_ok=True)
    with open(marker, 'w') as f:
        f.write('// Generated by java/stubs/tools/genstubs.py.\npackage stubs;\n\n'
                '/** Only the stubs name this type: their constructors that exist nowhere else take it. */\n'
                'public final class Marker {\n    private Marker() {\n    }\n}\n')
    digest = []
    for runtime in sorted(stubs):
        info, members, sup, interfaces = stubs[runtime]
        kind = 'missing' if info is None else ('interface' if info.access & ACC.ACC_INTERFACE else
                                               'enum' if info.access & ACC.ACC_ENUM else 'class')
        digest.append('class %s %s extends=%s implements=%s' % (runtime, kind, sup, ','.join(interfaces)))
        for m in sorted(members, key=lambda m: (m[0], m[2], m[3])):
            digest.append('  %s %s %s %s' % (m[0], m[2], m[3], 'static' if m[1] & ACC.ACC_STATIC else ''))
    for runtime in sorted(targets):
        info, members = src.members(runtime)
        digest.append('hooks %s' % runtime)
        for m in sorted(members, key=lambda m: (m[0], m[2], m[3])):
            if m[4] in targets[runtime] and not m[1] & ACC.ACC_SYNTHETIC:
                digest.append('  @%s %s %s  (%s)' % (m[0], m[2], m[3], m[4]))
    with open(os.path.join(out_dir, 'SIGNATURES.txt'), 'w', encoding='utf-8') as f:
        f.write('# What the stubs below declare (run-time names), the same for every version of a family.\n')
        f.write('\n'.join(digest) + '\n')
    if names_path:
        with open(names_path, 'w', encoding='utf-8') as f:
            f.write('# Minecraft %s: runtime name -> Mojang name, for the classes and members the stubs hold.\n' % version)
            for runtime in sorted(stubs):
                info, members, sup, interfaces = stubs[runtime]
                f.write('%s = %s\n' % (runtime, mojang_names.get(runtime, runtime)))
                for m in members:
                    f.write('    %s %s %s = %s %s\n' % (m[0], m[2], m[3], m[4], m[5]))
    return written, missing


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--jar', required=True)
    ap.add_argument('--mappings')
    ap.add_argument('--intermediary')
    ap.add_argument('--lib', action='append', default=[])
    ap.add_argument('--needed', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--names')
    ap.add_argument('--version', required=True)
    a = ap.parse_args()
    if bool(a.mappings) != bool(a.intermediary):
        sys.exit('--mappings and --intermediary go together')
    src = Source(a.jar, a.mappings, a.intermediary, a.lib)
    written, missing = generate(src, parse_needed(a.needed), a.out, a.names, a.version)
    print('%s: %d stub files%s' % (a.version, len(written), ('; not in this version: ' + ', '.join(missing)) if missing else ''))


if __name__ == '__main__':
    main()
