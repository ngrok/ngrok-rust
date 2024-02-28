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
	ngx_str_t forwards_to;
	Join *task;
} ngx_http_ngrok_srv_conf_t;

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

	return conf;
}

static char *
ngx_http_ngrok_merge_srv_conf(ngx_conf_t *cf, void *parent, void *child)
{
	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "cf: %d", cf);
	ngx_http_core_srv_conf_t *srv = ngx_http_conf_get_module_srv_conf(cf, ngx_ngrok_module);
	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "srv: %d", srv);
	ngx_http_ngrok_srv_conf_t *prev = parent;
	ngx_http_ngrok_srv_conf_t *conf = child;

	ngx_conf_merge_str_value(conf->domain, prev->domain, "");
	ngx_conf_merge_str_value(conf->domain, prev->domain, "");

	ngx_log_error(NGX_LOG_NOTICE, cf->log, 0, "listen: %d", srv->listen);

	bool needs_drop = false;

	if (conf->forwards_to.len != prev->forwards_to.len)
	{
		needs_drop = true;
	}

	// set -> something else
	if (
		conf->forwards_to.len != 0 &&
		prev->forwards_to.len != 0 &&
		ngx_strcmp(conf->forwards_to.data, prev->forwards_to.data) != 0)
	{
		needs_drop = true;
	}

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

	needs_drop = needs_drop && conf->task != NULL;

	if (needs_drop)
	{
		drop(conf->task);
		conf->task = NULL;
	}

	conf->task = start_ngrok((char *)conf->domain.data, "localhost:12345");

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