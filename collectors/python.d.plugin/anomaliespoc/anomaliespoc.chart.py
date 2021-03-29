# -*- coding: utf-8 -*-
# Description: anomaliespoc netdata python.d module
# Author: andrewm4894
# SPDX-License-Identifier: GPL-3.0-or-later

from json import loads

from bases.FrameworkServices.UrlService import UrlService

priority = 85

ORDER = [
    'chart_probs', 'chart_flags'
]

CHARTS = {
    'chart_probs': {
        'options': [None, 'Chart Probabilities', 'Chart Probability', 'chart probability', 'anomaliespoc.chart_probability', 'line'],
        'lines': []
    },
    'chart_flags': {
        'options': [None, 'Chart Flags', 'Chart Flag', 'chart flag', 'anomaliespoc.chart_flag', 'stacked'],
        'lines': []
    }
}


class Service(UrlService):
    def __init__(self, configuration=None, name=None):
        UrlService.__init__(self, configuration=configuration, name=name)
        self.order = ORDER
        self.definitions = CHARTS
        self.collected_dims = {'chart_probs': set(), 'chart_flags': set()}
        self.url = self.configuration.get('url', 'http://127.0.0.1:19999/api/v1/allmetrics?format=json')
        self.suffix = self.configuration.get('suffix', '_km')
        self.thold = self.configuration.get('thold', 50.0)

    def _get_data(self):
        raw_data = self._get_raw_data()
        if raw_data is None:
            return None

        raw_data = loads(raw_data)
        raw_data = {k: raw_data[k] for k in raw_data if k.endswith(self.suffix)}

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
