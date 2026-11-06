//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::EthApiError;

impl<N, Rpc> EthCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
{
}

impl<N, Rpc> Call for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
{
}

impl<N, Rpc> EstimateCall for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = EthApiError, Evm = N::Evm>,
{
}

#[cfg(test)]
mod call_x_integration_tests {
    use crate::{eth::helpers::types::EthRpcConverter, EthApi};
    use alloy_consensus::{Block, BlockBody, Header};
    use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
    use alloy_rpc_types_eth::{
        state::{AccountOverride, EvmOverrides, StateOverride},
        BlockId, BlockOverrides, TransactionRequest,
    };
    use reth_chainspec::{ChainSpec, ChainSpecProvider};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_eth_api::{
        helpers::{EthCall, EthState},
        node::RpcNodeCoreAdapter,
    };
    use reth_transaction_pool::test_utils::{testing_pool, TestPool};

    type Api = EthApi<
        RpcNodeCoreAdapter<MockEthProvider, TestPool, NoopNetwork, EthEvmConfig>,
        EthRpcConverter<ChainSpec>,
    >;

    fn api(gas_cap: u64) -> (Api, B256) {
        let provider = MockEthProvider::default();
        let hash = B256::repeat_byte(0x11);
        provider.add_block(
            hash,
            Block {
                header: Header {
                    number: 17_000_000,
                    timestamp: 1_680_000_000,
                    gas_limit: 1_000_000,
                    base_fee_per_gas: Some(1),
                    ..Default::default()
                },
                body: BlockBody::default(),
            },
        );
        let evm = EthEvmConfig::new(provider.chain_spec());
        (
            EthApi::builder(provider, testing_pool(), NoopNetwork::default(), evm)
                .gas_cap(gas_cap.into())
                .build(),
            hash,
        )
    }

    fn request_and_overrides(code: &str) -> (TransactionRequest, EvmOverrides) {
        let target = Address::repeat_byte(0x22);
        let state: StateOverride = [(
            target,
            AccountOverride { code: Some(code.parse::<Bytes>().unwrap()), ..Default::default() },
        )]
        .into_iter()
        .collect();
        (
            TransactionRequest { to: Some(TxKind::Call(target)), ..Default::default() },
            EvmOverrides::new(Some(state), None),
        )
    }

    #[tokio::test]
    async fn call_x_executes_state_override_bytecode_and_returns_logs() {
        let (api, hash) = api(100_000);
        // SSTORE/SLOAD 0x42, emit its word with LOG0, then return it.
        let (request, overrides) =
            request_and_overrides("0x604260005560005460005260206000a060206000f3");
        let response = EthCall::call_x(&api, request, Some(BlockId::hash(hash)), overrides, None)
            .await
            .unwrap();
        assert_eq!(response.status, 1);
        assert_eq!(response.block_hash, hash);
        assert_eq!(response.returns.len(), 32);
        assert_eq!(response.returns[31], 0x42);
        let logs = response.logs.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].data().data[31], 0x42);
        assert_eq!(logs[0].block_hash, Some(hash));
        assert!(response.used_gas > 21_000 && response.used_gas < 100_000);
        let stored = api
            .storage_at(Address::repeat_byte(0x22), U256::ZERO.into(), Some(BlockId::hash(hash)))
            .await
            .unwrap();
        assert_eq!(stored, B256::ZERO, "CallX must not persist SSTORE or state overrides");
    }

    #[tokio::test]
    async fn call_x_forwards_block_overrides_to_real_evm() {
        let (api, hash) = api(100_000);
        let (request, mut overrides) = request_and_overrides("0x4360005260206000f3");
        overrides.block =
            Some(Box::new(BlockOverrides { number: Some(U256::from(123)), ..Default::default() }));
        let response = EthCall::call_x(&api, request, Some(BlockId::hash(hash)), overrides, None)
            .await
            .unwrap();
        assert_eq!(response.status, 1);
        assert_eq!(U256::from_be_slice(&response.returns), U256::from(123));
        // Metadata identifies the requested state, even when NUMBER is overridden.
        assert_eq!(response.block_number, 17_000_000);
    }

    #[tokio::test]
    async fn call_x_applies_rpc_gas_cap_in_real_evm() {
        let (api, hash) = api(25_000);
        // Infinite JUMP loop must consume the configured cap and halt, not hang.
        let (mut request, overrides) = request_and_overrides("0x5b600056");
        request.gas = Some(50_000);
        let response = EthCall::call_x(&api, request, Some(BlockId::hash(hash)), overrides, None)
            .await
            .unwrap();
        assert_eq!(response.status, 0);
        assert_eq!(response.used_gas, 25_000);
        assert!(response.revert_error.unwrap().contains("OutOfGas"));
    }
    #[tokio::test]
    async fn call_x_pending_uses_pending_environment_without_a_canonical_hash() {
        let (api, hash) = api(100_000);
        let (request, overrides) = request_and_overrides("0x4360005260206000f3");
        let canonical = EthCall::call_x(
            &api,
            request.clone(),
            Some(BlockId::hash(hash)),
            overrides.clone(),
            None,
        )
        .await
        .unwrap();
        let pending = EthCall::call_x(&api, request, Some(BlockId::pending()), overrides, None)
            .await
            .unwrap();
        assert_eq!(canonical.block_number, 17_000_000);
        assert_eq!(U256::from_be_slice(&canonical.returns), U256::from(17_000_000));
        assert_eq!(pending.block_number, 17_000_001);
        assert_eq!(U256::from_be_slice(&pending.returns), U256::from(17_000_001));
        assert_eq!(pending.block_hash, B256::ZERO);
        assert_eq!(pending.flashblock_base_hash, None);
    }
}
