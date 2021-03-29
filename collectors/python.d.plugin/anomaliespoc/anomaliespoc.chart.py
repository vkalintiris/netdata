# -*- coding: utf-8 -*-
# Description: anomaliespoc netdata python.d module
# Author: andrewm4894
# SPDX-License-Identifier: GPL-3.0-or-later

from json import loads

from bases.FrameworkServices.UrlService import UrlService

priority = 85

ORDER = [
    'family_probs', 'family_flags', 'chart_probs', 'chart_flags'
]

CHARTS = {
    'family_probs': {
        'options': [None, 'Family Probabilities', 'Family Probability', 'family probabilities', 'anomaliespoc.family_probability', 'line'],
        'lines': []
    },
    'family_flags': {
        'options': [None, 'Family Flags', 'Family Flag', 'family flags', 'anomaliespoc.family_flag', 'stacked'],
        'lines': []
    },
    'chart_probs': {
        'options': [None, 'Chart Probabilities', 'Chart Probability', 'chart probabilities', 'anomaliespoc.chart_probability', 'line'],
        'lines': []
    },
    'chart_flags': {
        'options': [None, 'Chart Flags', 'Chart Flag', 'chart flags', 'anomaliespoc.chart_flag', 'stacked'],
        'lines': []
    },
}


class Service(UrlService):
    def __init__(self, configuration=None, name=None):
        UrlService.__init__(self, configuration=configuration, name=name)
        self.order = ORDER
        self.definitions = CHARTS
        self.collected_dims = {'chart_probs': set(), 'chart_flags': set()}
        self.url = self.configuration.get('url', 'http://127.0.0.1:19999/api/v1/allmetrics?format=json')
        self.suffix = self.configuration.get('suffix', '_km')
        self.thold = self.configuration.get('thold', 90.0)
        self.display_family = bool(self.configuration.get('display_family', True))

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
                    family_flags[family].append(chart_flags[chart])
            family_probs = {"{}_prob".format(f): round(sum(family_probs[f])/len(family_probs[f]), 2) for f in family_probs if len(family_probs[f]) > 0}
            family_flags = {"{}_flag".format(f): round(sum(family_flags[f])/len(family_flags[f]), 2) for f in family_flags if len(family_flags[f]) > 0}
            self.update_charts('family_probs', family_probs)
            self.update_charts('family_flags', family_flags)

            data = {**data, **family_probs, **family_flags}

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
