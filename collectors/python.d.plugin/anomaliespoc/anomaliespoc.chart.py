# -*- coding: utf-8 -*-
# Description: anomaliespoc netdata python.d module
# Author: andrewm4894
# SPDX-License-Identifier: GPL-3.0-or-later

from json import loads

from bases.FrameworkServices.UrlService import UrlService

priority = 85

ORDER = [
    'family_probs', 'family_flags', 'prefix_probs', 'prefix_flags', 'chart_probs', 'chart_flags'
]

CHARTS = {
    'family_probs': {
        'options': ['family_probs', 'Anomaly Probability', 'probability', 'family', 'anomaliespoc.family_probs', 'line'],
        'lines': []
    },
    'family_flags': {
        'options': ['family_flags', 'Anomaly Count', 'count', 'family', 'anomaliespoc.family_flags', 'stacked'],
        'lines': []
    },
    'prefix_probs': {
        'options': ['prefix_probs', 'Anomaly Probability', 'probability', 'prefix', 'anomaliespoc.prefix_probs', 'line'],
        'lines': []
    },
    'prefix_flags': {
        'options': ['prefix_flags', 'Anomaly Count', 'count', 'prefix', 'anomaliespoc.prefix_flags', 'stacked'],
        'lines': []
    },
    'chart_probs': {
        'options': ['chart_probs', 'Anomaly Probability', 'probability', 'chart', 'anomaliespoc.chart_probs', 'line'],
        'lines': []
    },
    'chart_flags': {
        'options': ['chart_flags', 'Anomaly Count', 'count', 'chart', 'anomaliespoc.chart_flags', 'stacked'],
        'lines': []
    },
}


class Service(UrlService):
    def __init__(self, configuration=None, name=None):
        UrlService.__init__(self, configuration=configuration, name=name)
        self.order = ORDER
        self.definitions = CHARTS
        self.collected_dims = {'chart_probs': set(), 'chart_flags': set(), 'family_probs': set(), 'family_flags': set(), 'prefix_probs': set(), 'prefix_flags': set()}
        self.url = self.configuration.get('url', 'http://127.0.0.1:19999/api/v1/allmetrics?format=json')
        self.suffix = self.configuration.get('suffix', '_km')
        self.thold = self.configuration.get('thold', 99.0)
        self.display_family = bool(self.configuration.get('display_family', True))
        self.display_prefix = bool(self.configuration.get('display_prefix', True))

    def _get_data(self):
        raw_data = self._get_raw_data()
        if raw_data is None:
            return None

        raw_data = loads(raw_data)
        chart_family_map = {c: raw_data[c]['family'] for c in raw_data}
        chart_families = list(set(chart_family_map.values()))
        raw_data = {k: raw_data[k] for k in raw_data if k.endswith(self.suffix)}

        # get chart level data
        chart_probs = {}
        chart_flags = {}
        for chart in raw_data:
            base_chart = chart.replace(self.suffix, '')
            anomaly_scores = [dim['value'] for dim in raw_data[chart]['dimensions'].values() if dim['value'] is not None]
            chart_probs[base_chart] = round(sum(anomaly_scores) / len(anomaly_scores), 2)
            chart_flags["{}_flag".format(base_chart)] = max([1 if score >= self.thold else 0 for score in anomaly_scores])        
        self.update_charts('chart_probs', chart_probs)
        self.update_charts('chart_flags', chart_flags)

        data = {**chart_probs, **chart_flags}

        # agg to family level
        if self.display_family:
            family_probs = {family: [] for family in chart_families}
            family_flags = {family: [] for family in chart_families}
            for chart in chart_probs:
                family = chart_family_map.get(chart, None)
                if family:
                    family_probs[family].append(chart_probs[chart])
                    family_flags[family].append(chart_flags["{}_flag".format(chart)])
            family_probs = {"{}_prob".format(f): round(sum(family_probs[f])/len(family_probs[f]), 2) for f in family_probs if len(family_probs[f]) > 0}
            family_flags = {"{}_flag".format(f): round(sum(family_flags[f])/len(family_flags[f]), 2) for f in family_flags if len(family_flags[f]) > 0}
            self.update_charts('family_probs', family_probs)
            self.update_charts('family_flags', family_flags)

            data = {**data, **family_probs, **family_flags}
        
        # agg to prefix or netdata 'type' level
        if self.display_prefix:
            chart_prefix_map = {k: k.split('.')[0] for k in chart_probs.keys()}
            chart_prefix_list = list(set(chart_prefix_map.values()))
            prefix_probs = {prefix: [] for prefix in chart_prefix_list}
            prefix_flags = {prefix: [] for prefix in chart_prefix_list}
            for chart in chart_probs:
                prefix = chart_prefix_map.get(chart, None)
                if prefix:
                    prefix_probs[prefix].append(chart_probs[chart])
                    prefix_flags[prefix].append(chart_flags["{}_flag".format(chart)])
            prefix_probs = {'{}.'.format(p): round(sum(prefix_probs[p])/len(prefix_probs[p]), 2) for p in prefix_probs if len(prefix_probs[p]) > 0}
            prefix_flags = {'{}._flag'.format(p): round(sum(prefix_flags[p])/len(prefix_flags[p]), 2) for p in prefix_flags if len(prefix_flags[p]) > 0}
            self.update_charts('prefix_probs', prefix_probs)
            self.update_charts('prefix_flags', prefix_flags)

            data = {**data, **prefix_probs, **prefix_flags}

        return data

    def update_charts(self, chart, data, algorithm='absolute', multiplier=1, divisor=1):
        if not self.charts:
            return

        for dim in data:
            if dim not in self.collected_dims[chart]:
                self.collected_dims[chart].add(dim)
                self.charts[chart].add_dimension([dim, dim, algorithm, multiplier, divisor])

        for dim in list(self.collected_dims[chart]):
            if dim not in data:
                self.collected_dims[chart].remove(dim)
                self.charts[chart].del_dimension(dim, hide=False)
