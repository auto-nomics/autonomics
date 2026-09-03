/**
 * @file 图像编码器模块入口
 *
 * 导出：
 *   - ImageEncoderWorkerPool：编码 Worker 池（并发编码像素数据）
 *   - EncodeImageRequest/Response：Worker 通信消息类型
 */
export * from './worker-pool';
export type { EncodeImageRequest, EncodeImageResponse } from './image-encoder-worker';
