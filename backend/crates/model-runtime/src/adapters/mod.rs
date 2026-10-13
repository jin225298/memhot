//! 协议 adapter：HTTP/SSE 编解码与供应商兼容差异。
//!
//! 每个 adapter 只负责一种协议的请求构造、响应解析与错误分类；
//! 不执行产品工具、不做业务降级。重试/替代属于 execution 层。

pub mod chat_completions;
