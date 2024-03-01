/*
 * Copyright (C) Author.
 */

#include <ngx_core.h>
#include <ngx_config.h>

#include <ngx_http.h>
#include <ngx_http_config.h>

#include <ngx_string.h>

#include "ngrok.h"

typedef struct
{
	ngx_str_t domain;
	ngx_str_t policy_file;
	Join *task;
} ngx_http_ngrok_srv_conf_t;

char *add_listener(ngx_conf_t *cf, ngx_http_core_srv_conf_t *cscf, uint16_t port);

static char *ngx_ngrok_enable(ngx_conf_t *cf, void *post, void *data);
static ngx_conf_post_t ngx_ngrok_enable_post = {ngx_ngrok_enable};

static void *
ngx_http_ngrok_create_srv_conf(ngx_conf_t *cf);
static char *
ngx_http_ngrok_merge_srv_conf(ngx_conf_t *cf, void *parent, void *child);

static ngx_command_t ngx_ngrok_commands[] = {

	{ngx_string("ngrok"),
	 NGX_HTTP_SRV_CONF | NGX_CONF_TAKE1,
	 ngx_conf_set_str_slot,
	 NGX_HTTP_SRV_CONF_OFFSET,
	 offsetof(ngx_http_ngrok_srv_conf_t, domain),
	 &ngx_ngrok_enable_post},

	{ngx_string("ngrok_policy_file"),
	 NGX_HTTP_SRV_CONF | NGX_CONF_TAKE1,
	 ngx_conf_set_str_slot,
	 NGX_HTTP_SRV_CONF_OFFSET,
	 offsetof(ngx_http_ngrok_srv_conf_t, policy_file),
	 &ngx_ngrok_enable_post},

	ngx_null_command};

static ngx_http_module_t ngx_http_ngrok_module_ctx = {
	NULL, /* preconfiguration */
	NULL, /* postconfiguration */

	NULL, /* create main configuration */
	NULL, /* init main configuration */

	ngx_http_ngrok_create_srv_conf, /* create server configuration */
	ngx_http_ngrok_merge_srv_conf,	/* merge server configuration */

	NULL, /* create location configuration */
	NULL  /* merge location configuration */
};

ngx_module_t ngx_ngrok_module = {
	NGX_MODULE_V1,
	&ngx_http_ngrok_module_ctx, /* module context */
	ngx_ngrok_commands,			/* module directives */
	NGX_HTTP_MODULE,			/* module type */
	NULL,						/* init master */
	NULL,						/* init module */
	NULL,						/* init process */
	NULL,						/* init thread */
	NULL,						/* exit thread */
	NULL,						/* exit process */
	NULL,						/* exit master */
	NGX_MODULE_V1_PADDING};

static void *
ngx_http_ngrok_create_srv_conf(ngx_conf_t *cf)
{
	ngx_http_ngrok_srv_conf_t *conf;

	conf = ngx_pcalloc(cf->pool, sizeof(ngx_http_ngrok_srv_conf_t));
	if (conf == NULL)
	{
		return NULL;
	}

	ngx_str_t ns = ngx_null_string;
	conf->domain = ns;
	conf->policy_file = ns;

	return conf;
}

static char *
ngx_http_ngrok_merge_srv_conf(ngx_conf_t *cf, void *parent, void *child)
{
	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "cf: %d", cf);
	ngx_http_core_srv_conf_t *core_srv = ngx_http_conf_get_module_srv_conf(cf, ngx_http_core_module);
	ngx_http_core_srv_conf_t *srv = ngx_http_conf_get_module_srv_conf(cf, ngx_ngrok_module);
	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "srv: %d", srv);
	ngx_http_ngrok_srv_conf_t *prev = parent;
	ngx_http_ngrok_srv_conf_t *conf = child;

	// Generate a port in the ephemeral TCP range.
	uint16_t fwd_port = 1024 + rand() % (65535 - 1024);
	if (add_listener(cf, core_srv, fwd_port) != NGX_OK)
	{
		ngx_log_error(NGX_LOG_ERR, cf->log, 0, "failed to bind listener for ngrok");
		return NGX_CONF_ERROR;
	}
	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "added listener");

	ngx_conf_merge_str_value(conf->domain, prev->domain, "");

	ngx_conf_merge_str_value(conf->policy_file, prev->policy_file, "");

	bool needs_drop = false;

	if (conf->domain.len != prev->domain.len)
	{
		needs_drop = true;
	}

	// set -> something else
	if (
		conf->domain.len != 0 &&
		prev->domain.len != 0 &&
		ngx_strcmp(conf->domain.data, prev->domain.data) != 0)
	{
		needs_drop = true;
	}

	if (conf->policy_file.len != prev->policy_file.len)
	{
		needs_drop = true;
	}

	// set -> something else
	if (
		conf->policy_file.len != 0 &&
		prev->policy_file.len != 0 &&
		ngx_strcmp(conf->policy_file.data, prev->policy_file.data) != 0)
	{
		needs_drop = true;
	}

	needs_drop = needs_drop && conf->task != NULL;

	if (needs_drop)
	{
		drop(conf->task);
		conf->task = NULL;
	}

	if (conf->domain.len != 0 || conf->policy_file.len != 0)
	{
		conf->task = start_ngrok((char *)conf->domain.data, fwd_port, (char *)conf->policy_file.data);
	}

	return NULL;
}

static char *
ngx_ngrok_enable(ngx_conf_t *cf, void *post, void *data)
{
	ngx_str_t *fp = data;

	if (fp->data != NULL)
	{
		ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "ngrok is enabled: %s", fp->data);
	}

	return NGX_CONF_OK;
}

char *add_listener(ngx_conf_t *cf, ngx_http_core_srv_conf_t *cscf, uint16_t port)
{
	u_char *p;
	struct sockaddr_in *sin;
	ngx_http_listen_opt_t lsopt;
	size_t len;

	ngx_memzero(&lsopt, sizeof(ngx_http_listen_opt_t));

	p = ngx_pcalloc(cf->pool, sizeof(struct sockaddr_in));
	if (p == NULL)
	{
		return NGX_CONF_ERROR;
	}

	lsopt.sockaddr = (struct sockaddr *)p;

	sin = (struct sockaddr_in *)p;

	sin->sin_family = AF_INET;
	sin->sin_port = htons(port);
	sin->sin_addr.s_addr = htonl(INADDR_LOOPBACK);

	lsopt.socklen = sizeof(struct sockaddr_in);

	lsopt.backlog = NGX_LISTEN_BACKLOG;
	lsopt.rcvbuf = -1;
	lsopt.sndbuf = -1;
#if (NGX_HAVE_SETFIB)
	lsopt.setfib = -1;
#endif
#if (NGX_HAVE_TCP_FASTOPEN)
	lsopt.fastopen = -1;
#endif
	lsopt.wildcard = 1;

	lsopt.proxy_protocol = 1;
	lsopt.http2 = 1;

	len = NGX_INET_ADDRSTRLEN + sizeof(":65535") - 1;

	p = ngx_pnalloc(cf->pool, len);
	if (p == NULL)
	{
		return NGX_CONF_ERROR;
	}

	lsopt.addr_text.data = p;
	lsopt.addr_text.len = ngx_sock_ntop(lsopt.sockaddr, lsopt.socklen, p,
										len, 1);

	if (ngx_http_add_listen(cf, cscf, &lsopt) != NGX_OK)
	{
		return NGX_CONF_ERROR;
	}

	return NULL;
}