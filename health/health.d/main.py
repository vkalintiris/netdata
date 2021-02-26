#!/usr/bin/env python

import collections
import csv
import functools
import io
import os
import sys

import click

# v2.4.7
import pyparsing as pp

# Column names that we want to extract from the CSV file
# Add/remove/rename according to what you need.
SOURCE_FILE_COL = "source file"
ALERT_NAME_COL = "name (Alert Name)"
CLASS_COL = "Class"
COMPONENT_COL = "Component"
TYPE_COL = "Type \n(Saturation, Traffic, Latency, Errors)"

class ConfigFile:
    KEYWORDS = [
        "alarm", "template", "on", "hosts", "os", "families", "plugin",
        "module", "lookup", "calc", "every", "green", "red", "warn", "crit",
        "exec", "to", "units", "info", "delay", "options", "repeat",
        "host labels",
        "class", "component", "type",
    ]

    def __init__(self, path, alert_name, cls, component, ty):
        self.path = path
        self.alert_name = alert_name
        self.cls = cls
        self.component = component
        self.ty = ty

        self.lines = []

    def addNonDictLine(self, s, loc, res):
        self.lines.append(''.join(res))

    def addKey(self, s, loc, res):
        self.lines.append(''.join(res))

    def addValue(self, s, loc, res):
        key = self.lines.pop()
        self.lines.append((key, ''.join(res)))

    def parse(self):
        EmptyLine = pp.LineStart() + pp.Regex('[ \t]*') + pp.LineEnd()
        CommentLine  = pp.Regex('[ \t]*') + pp.Literal('#') + pp.Regex('[^\n]*') + pp.LineEnd()
        NonDictLine = EmptyLine ^ CommentLine
        cb = functools.partial(self.addNonDictLine, self)
        NonDictLine.addParseAction(cb)

        KeyExpr = pp.Or([pp.Literal(kw) for kw in self.KEYWORDS])
        KeyExpr.addParseAction(functools.partial(self.addKey, self))

        ValueExpr = pp.Regex(r'(?:[^\n]*\\\n)*[^\n]*') + pp.LineEnd()
        ValueExpr.addParseAction(functools.partial(self.addValue, self))

        KVLine = pp.Regex('[ \t]*') + KeyExpr + pp.Regex('[ \t]*:[ \t]*') + ValueExpr

        LineExpr = NonDictLine ^ KVLine
        FileExpr = pp.OneOrMore(LineExpr)

        FileExpr.leaveWhitespace()
        FileExpr.parseFile(self.path)

    def write_kv(self, fp, idx):
        od = collections.OrderedDict()
        while idx < len(self.lines) and isinstance(self.lines[idx], tuple):
            k, v = self.lines[idx]
            od[k] = v
            idx += 1

            if k in ('template', 'alarm') and self.alert_name in v:
                if self.cls:
                    od['class'] = self.cls + '\n'
                if self.component:
                    od['component'] = self.component + '\n'
                if self.ty:
                    od['type'] = self.ty + '\n'


        max_key_len = max(len(x) for x in od.keys())
        for k, v in od.items():
            fp.write(k.rjust(max_key_len) + ': ')
            fp.write(v)

        return idx


    def __str__(self):
        fp = io.StringIO()

        idx = 0
        while idx < len(self.lines):
            line = self.lines[idx]

            if isinstance(line, str):
                fp.write(line)
                idx += 1
            elif isinstance(line, tuple):
                idx = self.write_kv(fp, idx)
            else:
                raise Exception("Unknown line type")

        s = fp.getvalue()
        fp.close()
        return s


@click.command()
@click.option(
    '--csv-file',
    type=click.File('r'),
    required=True,
    help='CSV file exported from Google Sheets',
)
@click.option(
    '--health-conf-dir',
    type=click.Path(exists=True, file_okay=False, resolve_path=True),
    required=True,
    help='Directory containing health *.conf files',
)
def main(csv_file, health_conf_dir):
    print(csv_file.name)
    csv_reader = csv.DictReader(csv_file)
    for row in csv_reader:
        alert_name, source_file = row[ALERT_NAME_COL], row[SOURCE_FILE_COL]
        cls, component, ty = row[CLASS_COL], row[COMPONENT_COL], row[TYPE_COL]

        source_file = source_file.strip()
        alert_name = alert_name.strip()
        cls = cls.strip()
        component = component.strip()
        ty = ty.strip()

        p = os.path.abspath(
            os.path.join(health_conf_dir, source_file)
        )
        if not os.path.isfile(p):
            raise ValueError(f"Alarm config file {p} does not exist")

        print(f"Parsing {p}")
        cfg = ConfigFile(p, alert_name, cls, component, ty)
        cfg.parse()

        with open(p, 'w') as fp:
            fp.write(str(cfg))


if __name__ == '__main__':
    main()
