# sup run: {{ view.counts }}

| | Job | Time | Note |
| --- | --- | ---: | --- |
{% for job in view.jobs -%}
| {{ job.icon }} | {{ job.name }} | {{ job.time }} | {{ job.markdown_note() }} |
{% endfor -%}
{% if let Some(logs) = view.log_dir %}
Logs: `{{ logs.full }}`
{% endif -%}
{% for failure in view.failures %}
## ✗ {{ failure.label }} (`{{ failure.name }}`)

{% if let Some(code) = failure.exit_code -%}
- Exit code: {{ code }}
{% endif -%}
{% if !failure.reason.is_empty() -%}
- Reason: {{ failure.reason }}
{% endif -%}
{% if let Some(log) = failure.log -%}
- Log: `{{ log.full }}`
{% endif -%}
{% if let Some(error) = failure.tail_error -%}
- {{ error }}
{% else if !failure.tail.is_empty() %}
````text
{% for line in failure.tail -%}
{{ line }}
{% endfor -%}
````
{% endif -%}
{% endfor -%}
