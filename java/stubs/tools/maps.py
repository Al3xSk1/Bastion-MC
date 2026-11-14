"""Mojang (ProGuard) client mappings joined with Fabric intermediary (tiny v2), per version."""
import zipfile, re

PRIM = {'int': 'I', 'float': 'F', 'double': 'D', 'long': 'J', 'boolean': 'Z', 'byte': 'B',
        'char': 'C', 'short': 'S', 'void': 'V'}

def jtype_to_desc(t, classmap=None):
    dims = 0
    while t.endswith('[]'):
        dims += 1
        t = t[:-2]
    if t in PRIM:
        d = PRIM[t]
    else:
        internal = t.replace('.', '/')
        if classmap is not None:
            internal = classmap.get(internal, internal)
        d = 'L' + internal + ';'
    return '[' * dims + d

def split_desc(desc):
    """'(ILfoo;[J)V' -> (['I','Lfoo;','[J'], 'V')"""
    assert desc[0] == '('
    i, args = 1, []
    while desc[i] != ')':
        j = i
        while desc[j] == '[':
            j += 1
        if desc[j] == 'L':
            j = desc.index(';', j)
        args.append(desc[i:j + 1])
        i = j + 1
    return args, desc[i + 1:]

def map_desc(desc, classmap):
    def one(d):
        k = 0
        while d[k] == '[':
            k += 1
        if d[k] == 'L':
            name = d[k + 1:-1]
            return d[:k] + 'L' + classmap.get(name, name) + ';'
        return d
    if desc.startswith('('):
        args, ret = split_desc(desc)
        return '(' + ''.join(one(a) for a in args) + ')' + one(ret)
    return one(desc)

class Mojang:
    def __init__(self, path):
        self.classes = {}   # named internal -> dict(obf, fields: [(name, type, obf)], methods: [(name, params, ret, obf)])
        cur = None
        for line in open(path, encoding='utf-8'):
            if line.startswith('#') or not line.strip():
                continue
            if not line.startswith(' '):
                named, obf = line.rstrip().rstrip(':').split(' -> ')
                cur = {'obf': obf.replace('.', '/'), 'fields': [], 'methods': []}
                self.classes[named.replace('.', '/')] = cur
                continue
            body = line.strip()
            left, obf = body.split(' -> ')
            m = re.match(r'(?:\d+:\d+:)?(\S+) ([^ (]+)\(([^)]*)\)(?::\d+:\d+)?$', left)
            if m:
                ret, name, params = m.groups()
                params = [p for p in params.split(',') if p]
                cur['methods'].append((name, params, ret, obf))
            else:
                typ, name = left.split(' ')
                cur['fields'].append((name, typ, obf))
        self.named_to_obf = {n: c['obf'] for n, c in self.classes.items()}
        self.obf_to_named = {c['obf']: n for n, c in self.classes.items()}

class Tiny:
    def __init__(self, jarpath):
        z = zipfile.ZipFile(jarpath)
        text = z.read('mappings/mappings.tiny').decode()
        lines = text.split('\n')
        header = lines[0].split('\t')
        assert header[0] == 'tiny' and header[3] == 'official' and header[4] == 'intermediary', header
        self.classes = {}  # obf -> dict(name, fields {(obf, desc): iname}, methods {...})
        cur = None
        for line in lines[1:]:
            if not line:
                continue
            parts = line.split('\t')
            if parts[0] == 'c':
                cur = {'name': parts[2], 'fields': {}, 'methods': {}}
                self.classes[parts[1]] = cur
            elif len(parts) >= 5 and parts[1] == 'm':
                cur['methods'][(parts[3], parts[2])] = parts[4]
            elif len(parts) >= 5 and parts[1] == 'f':
                cur['fields'][(parts[3], parts[2])] = parts[4]
        self.obf_to_int = {o: c['name'] for o, c in self.classes.items()}

class Joined:
    """Named (Mojang) -> intermediary, for one version."""
    def __init__(self, mojang_path, tiny_jar):
        self.m = Mojang(mojang_path)
        self.t = Tiny(tiny_jar)
        self.named_to_int = {}
        for named, c in self.m.classes.items():
            self.named_to_int[named] = self.t.obf_to_int.get(c['obf'], c['obf'])

    def cls(self, named):
        return self.named_to_int.get(named)

    def members(self, named):
        """[(kind, name, named_desc, int_name, int_desc)] declared in `named`."""
        c = self.m.classes.get(named)
        if c is None:
            return None
        tc = self.t.classes.get(c['obf'], {'fields': {}, 'methods': {}})
        out = []
        for name, typ, obf in c['fields']:
            nd = jtype_to_desc(typ)
            od = jtype_to_desc(typ, self.m.named_to_obf)
            iname = tc['fields'].get((obf, od), name if name == obf else obf)
            out.append(('f', name, nd, iname, map_desc(od, self.t.obf_to_int)))
        for name, params, ret, obf in c['methods']:
            nd = '(' + ''.join(jtype_to_desc(p) for p in params) + ')' + jtype_to_desc(ret)
            od = '(' + ''.join(jtype_to_desc(p, self.m.named_to_obf) for p in params) + ')' + jtype_to_desc(ret, self.m.named_to_obf)
            if name in ('<init>', '<clinit>'):
                iname = name
            else:
                iname = tc['methods'].get((obf, od), obf)
            out.append(('m', name, nd, iname, map_desc(od, self.t.obf_to_int)))
        return out
