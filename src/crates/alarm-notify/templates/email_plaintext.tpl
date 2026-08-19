Content-Type: text/plain; encoding=${EMAIL_CHARSET}
Content-Disposition: inline
Content-Transfer-Encoding: 8bit

${host} ${status_message}

${alarm} ${info}
${raised_for}

Chart   : ${chart}
Severity: ${severity}
URL     : ${goto_url}
Source  : ${src}
Date    : ${date}
Notification generated on ${host}

Evaluated Expression :  ${calc_expression}
Expression Variables :  ${calc_param_values}

The host has ${total_warnings} WARNING and ${total_critical} CRITICAL alarm(s) raised.
