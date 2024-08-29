pub mod block_body;
pub mod block_header;
pub mod block_height;
pub mod block_info;
pub mod block_kernel;
pub mod block_selector;
pub mod mutator_set_update;
pub mod transfer_block;
pub mod validity;

use std::cmp::max;
use std::sync::OnceLock;

use block_body::BlockBody;
use block_header::BlockHeader;
use block_header::MINIMUM_DIFFICULTY;
use block_header::TARGET_BLOCK_INTERVAL;
use block_header::TARGET_DIFFICULTY_U32_SIZE;
use block_height::BlockHeight;
use block_kernel::BlockKernel;
use get_size::GetSize;
use itertools::Itertools;
use mutator_set_update::MutatorSetUpdate;
use num_bigint::BigUint;
use num_traits::abs;
use num_traits::Zero;
use serde::Deserialize;
use serde::Serialize;
use tasm_lib::triton_vm::proof::Proof;
use tasm_lib::twenty_first::util_types::mmr::mmr_accumulator::MmrAccumulator;
use tasm_lib::twenty_first::util_types::mmr::mmr_trait::Mmr;
use tracing::debug;
use tracing::error;
use tracing::warn;
use transfer_block::ProofType;
use transfer_block::TransferBlock;
use twenty_first::amount::u32s::U32s;
use twenty_first::math::b_field_element::BFieldElement;
use twenty_first::math::bfield_codec::BFieldCodec;
use twenty_first::math::digest::Digest;
use twenty_first::math::tip5::DIGEST_LENGTH;
use twenty_first::util_types::algebraic_hasher::AlgebraicHasher;

use crate::config_models::network::Network;
use crate::models::blockchain::shared::Hash;
use crate::models::consensus::mast_hash::MastHash;
use crate::models::consensus::timestamp::Timestamp;
use crate::models::consensus::ValidityAstType;
use crate::models::consensus::ValidityTree;
use crate::models::consensus::WitnessType;
use crate::models::state::wallet::address::ReceivingAddress;
use crate::models::state::wallet::WalletSecret;
use crate::prelude::twenty_first;
use crate::util_types::mutator_set::commit;
use crate::util_types::mutator_set::mutator_set_accumulator::MutatorSetAccumulator;

use super::transaction::transaction_kernel::TransactionKernel;
use super::transaction::utxo::Utxo;
use super::transaction::validity::TransactionValidationLogic;
use super::transaction::Transaction;
use super::type_scripts::neptune_coins::NeptuneCoins;
use super::type_scripts::time_lock::TimeLock;

/// All blocks have proofs except the genesis block
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BFieldCodec, GetSize)]
pub enum BlockType {
    Genesis,
    Standard(ProofType),
}

/// Public fields of `Block` are read-only, enforced by #[readonly::make].
/// Modifications are possible only through `Block` methods.
///
/// Example:
///
/// test: verify that compile fails on an attempt to mutate block
/// internals directly (bypassing encapsulation)
///
/// ```compile_fail,E0594
/// use neptune_core::models::blockchain::block::Block;
/// use neptune_core::config_models::network::Network;
/// use neptune_core::prelude::twenty_first::math::b_field_element::BFieldElement;
///
/// let mut block = Block::genesis_block(Network::RegTest);
///
/// let height = block.kernel.header.height;
///
/// let one = BFieldElement::from(1u32);
/// let nonce = [one, one, one];
///
/// // this line fails to compile because we try to
/// // mutate an internal field.
/// block.kernel.header.nonce = nonce;
/// ```

// ## About the private `digest` field:
//
// The `digest` field represents the `Block` hash.  It is an optimization so
// that the hash can be lazily computed at most once (per modification).
//
// It is wrapped in `OnceLock<_>` for interior mutability because (a) the hash()
// method is used in many methods that are `&self` and (b) because `Block` is
// passed between tasks/threads, and thus `Rc<RefCell<_>>` is not an option.
//
// The field must be reset whenever the Block is modified.  As such, we should
// not permit direct modification of internal fields, particularly `kernel`
//
// Therefore `[readonly::make]` is used to make public `Block` fields read-only
// (not mutable) outside of this module.  All methods that modify Block also
// reset the `digest` field.
//
// We manually implement `PartialEq` and `Eq` so that digest field will not be
// compared.  Otherwise, we could have identical blocks except one has
// initialized digest field and the other has not.
//
// The field should not be serialized, so it has the `#[serde(skip)]` attribute.
// Upon deserialization, the field will have Digest::default() which is desired
// so that the digest will be recomputed if/when hash() is called.
//
// We likewise skip the field for `BFieldCodec`, and `GetSize` because there
// exist no impls for `OnceLock<_>` so derive fails.
//
// A unit test-suite exists in module tests::digest_encapsulation.
#[readonly::make]
#[derive(Clone, Debug, Serialize, Deserialize, BFieldCodec, GetSize)]
pub struct Block {
    /// Everything but the proof
    pub kernel: BlockKernel,

    /// type of block: Genesis, or Standard
    pub block_type: BlockType,

    // this is only here as an optimization for Block::hash()
    // so that we lazily compute the hash at most once.
    #[serde(skip)]
    #[bfield_codec(ignore)]
    #[get_size(ignore)]
    digest: OnceLock<Digest>,
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        // TBD: is it faster overall to compare hashes or equality
        // of kernel and blocktype fields?
        // In the (common?) case where hash has already been
        // computed for both `Block` comparing hash equality
        // should be faster.
        self.hash() == other.hash()
    }
}
impl Eq for Block {}

impl From<TransferBlock> for Block {
    fn from(t_block: TransferBlock) -> Self {
        let kernel = BlockKernel {
            header: t_block.header,
            body: t_block.body,
        };
        Self {
            digest: Default::default(), // calc'd in hash()
            kernel,
            block_type: BlockType::Standard(t_block.proof_type),
        }
    }
}

impl From<Block> for TransferBlock {
    fn from(block: Block) -> Self {
        let proof_type = match block.block_type {
            BlockType::Standard(pt) => pt,
            BlockType::Genesis => {
                error!("The Genesis block cannot be transferred");
                panic!()
            }
        };
        Self {
            header: block.kernel.header,
            body: block.kernel.body,
            proof_type,
        }
    }
}

impl Block {
    /// Returns the block Digest
    ///
    /// performance note:
    ///
    /// The digest is never computed until hash() is called.  Subsequent calls
    /// will not recompute it unless the Block was modified since the last call.
    #[inline]
    pub fn hash(&self) -> Digest {
        *self.digest.get_or_init(|| self.kernel.mast_hash())
    }

    #[inline]
    fn unset_digest(&mut self) {
        // note: this replaces the OnceLock so the digest will be calc'd in hash()
        self.digest = Default::default();
    }

    /// sets header header nonce.
    ///
    /// note: this causes block digest to change.
    #[inline]
    pub fn set_header_nonce(&mut self, nonce: [BFieldElement; 3]) {
        self.kernel.header.nonce = nonce;
        self.unset_digest();
    }

    /// sets header timestamp and difficulty.
    ///
    /// These must be set as a pair because the difficulty depends
    /// on the timestamp, and may change with it.
    ///
    /// note: this causes block digest to change.
    #[inline]
    pub fn set_header_timestamp_and_difficulty(
        &mut self,
        timestamp: Timestamp,
        difficulty: U32s<5>,
    ) {
        self.kernel.header.timestamp = timestamp;
        self.kernel.header.difficulty = difficulty;

        self.unset_digest();
    }

    #[inline]
    pub fn header(&self) -> &BlockHeader {
        &self.kernel.header
    }

    #[inline]
    pub fn body(&self) -> &BlockBody {
        &self.kernel.body
    }

    /// note: this causes block digest to change to that of the new block.
    #[inline]
    pub fn set_block(&mut self, block: Block) {
        self.kernel.header = block.kernel.header;
        self.kernel.body = block.kernel.body;
        self.digest = block.digest;
    }

    pub fn get_mining_reward(block_height: BlockHeight) -> NeptuneCoins {
        let mut reward: NeptuneCoins = NeptuneCoins::new(100);
        let generation = block_height.get_generation();
        for _ in 0..generation {
            reward.div_two()
        }

        reward
    }

    pub fn genesis_block(network: Network) -> Self {
        let mut genesis_mutator_set = MutatorSetAccumulator::default();
        let mut ms_update = MutatorSetUpdate::default();

        let premine_distribution = Self::premine_distribution(network);
        let total_premine_amount = premine_distribution
            .iter()
            .map(|(_receiving_address, amount)| *amount)
            .sum();

        let mut genesis_coinbase_tx = Transaction {
            kernel: TransactionKernel {
                inputs: vec![],
                outputs: vec![],
                fee: NeptuneCoins::new(0),
                timestamp: network.launch_date(),
                public_announcements: vec![],
                coinbase: Some(total_premine_amount),
                mutator_set_hash: MutatorSetAccumulator::default().hash(),
            },
            witness: TransactionValidationLogic {
                vast: ValidityTree {
                    vast_type: ValidityAstType::Axiom,
                    witness_type: WitnessType::Faith,
                },
                maybe_primitive_witness: None,
            },
        };

        for ((receiving_address, _amount), utxo) in premine_distribution
            .iter()
            .zip(Self::premine_utxos(network))
        {
            let utxo_digest = Hash::hash(&utxo);
            // generate randomness for mutator set commitment
            // Sender randomness cannot be random because there is no sender.
            let bad_randomness = Digest::default();
            let receiver_digest = receiving_address.privacy_digest();

            // Add pre-mine UTXO to MutatorSet
            let addition_record = commit(utxo_digest, bad_randomness, receiver_digest);
            ms_update.additions.push(addition_record);
            genesis_mutator_set.add(&addition_record);

            // Add pre-mine UTXO + commitment to coinbase transaction
            genesis_coinbase_tx.kernel.outputs.push(addition_record)
        }

        let body: BlockBody = BlockBody {
            transaction: genesis_coinbase_tx,
            mutator_set_accumulator: genesis_mutator_set.clone(),
            block_mmr_accumulator: MmrAccumulator::new(vec![]),
            lock_free_mmr_accumulator: MmrAccumulator::new(vec![]),
            uncle_blocks: vec![],
        };

        let header: BlockHeader = BlockHeader {
            version: BFieldElement::zero(),
            height: BFieldElement::zero().into(),
            prev_block_digest: Default::default(),
            timestamp: network.launch_date(),
            // to be set to something difficult to predict ahead of time
            nonce: [
                BFieldElement::zero(),
                BFieldElement::zero(),
                BFieldElement::zero(),
            ],
            max_block_size: 10_000,
            proof_of_work_line: U32s::zero(),
            proof_of_work_family: U32s::zero(),
            difficulty: MINIMUM_DIFFICULTY.into(),
        };

        Self::new(header, body, BlockType::Genesis)
    }

    fn premine_distribution(_network: Network) -> Vec<(ReceivingAddress, NeptuneCoins)> {
        // The premine UTXOs can be hardcoded here.
        let authority_wallet = WalletSecret::devnet_wallet();
        let authority_receiving_address = authority_wallet
            .nth_generation_spending_key(0)
            .to_address()
            .into();
        vec![
            // chiefly for testing; anyone can access these coins by generating the devnet wallet as above
            (authority_receiving_address, NeptuneCoins::new(20000)),

            // also for testing, but for internal use only
            (ReceivingAddress::from_bech32m("nolgam1t6h52ck34mkvvmkk8nnzesf5sdcks3mlj23k8hgp5gc39qaxx76qnltllx465np340n0mf9zrv2e04425q69xlhjgy35v3zu7jmnljev9n38t2a86d9sqq84g8y9egy23etpkewp4ad64s66qq9cruyp0r0vz50urcalgxerv6xcuet6j5tcdx6tqm6d772dxu29r6kq8mkzkyrc07072rlvkx4tkmwy29aqq8qmwwd0n4at3qllgvd427um3jsjed696rddert6dzlamqtn66mz997xt8nslrq8dqvl2nx4k7vu50ul7584m7243pdzdczgnxcd0a8q8aspfd66s5spaa5nk8sqfh29htak8lzf853edgqw99fu4v4ess3d9z0gcqjpclks9p2w5srta9n65r5w2rj89jmagtuklz838lj726frzdvlfj7t992hz8n355raxy2xnm4fpfr20zvk38caatsd74lzx370mfhqrakf6achx5fv858wpchjlmu3h55s5kqkmfu0zhw05wfx7meu33fnmw0fju6p0m940nfrsqkv0e8q25g3sgjk4t0qfun0st7h2k4ef6cau3zyrc5dsqukvzwd85kxxf9ksk6jw7k5ny7wku6wf90mx5xyd7p6q5w6eu4wxxfeqryyfw2rdprr7fkzg9hrt97s4hn9cgpr6qz8x0j59gm885ekde9czanpksqq0c0kmefzfha3lqw8v2xeme5nmf93u59z8luq4wprlxj6v7mpp80t3sjvmv3a6t2kxsh9qaw9spj789ft8jswzm2kmfywxn80caccqf4d38kkjg5ahdrkmfvec242rg47ewzwsfy590hxyvz5v3dpg2a99vwc20a749rmygj74k2uw794t66dz0n9chmhd47gg84y8qc62jvjl8num4j7s2c0gtc88t3pun4zwuq55vf66mg4n8urn50lm7ww4he5x5ya4yyaqlrn2ag5sdnqt46magvw90hh9chyq3q9qc36pq4tattn6lvzfjp9trxuske84yttf6pa3le9z0z8y06gv7925dshhfjn4y5y3aykfg2g7ujrlly8dgpk3srlvq0zmdvgu5jsxwqvngvp6fh6he8fyrlqgrs58qklrg3zyu2jl9nrp2hdvj3hwh29fk5mjl9tpjx0tnyys5gkqlvxxhel4yh53ms0rxpkw3sa6teqgpe4yej5sk7edyqn7w8xr4mgm2asww53gzv95fwpud7mzg4rrnpvdk40m0vna8w8y0w9y240r6m7ja58gfk3stfra9qsm0lt7npkv4w0ghzypdrrg04kp7kkepnm4qmwmjxdg2tx3ejtdmzp0w08alv7x3zxgxsu35yhlvrnkpl9mxgejkfcxdgccper4f7llaaux9hcpul5uy47lhr065qwkgxc6jfylq5raqeczryz089syr4aj7z908e4e3t49qd40x3ueyrgxcdj37dkd5ysezj45kgtv546e7m3fj8ga920lztrgmmx0a98qwnk2ep5k9qh2x05mm5snu5d88lm4lrad8hc639jx97hrx9mywkw6c7yvj9jv0mjmsq0xqpqt0kc4hsh24kndhtsc0ezfzw9h79mjw239s804t2f4jucd3x57mvvnsyp82xy9jvp4yzlq5qhrpu87frkfwkx62r8rjsdkdlx4yhss2ly4q8425ta3je6rym35lapxesd9dhsj44pfhmq92g4tmfr8qnajpn2cgj8ngtzrkc9ygsvx76633p8ksru7g8cda5dfnhf50ax47rde5fhnk8dt7k5sltkhknha697gyqsjg4hytslxmaazdjqj4earaf098uz6gpcgu27zsy4v5arc3vjmum90ngf8e00exjr4nsqs3wr4w93h42ucnllyu5ck09yundjkjqsqetrhzvc3q0smssg6vcw9hlns363grqyt92azpvml632wffpuq5wtsh9vxwdse0g0w0wl3e320hnp3vlmzde3c8xa42yye90gnmmyjdq5atmlnulga4pcapk4t6ut82w057ed3rawx42vn7rl5kzyg84cvulg8yfjeu3ff0wprytkhk85dr63u9elq5ju0c9vd2yyjkqnhxh6xwxnt4nw32pefm9aengdasjn7lsyaeldz93spfnn02uke83xkwytj0wkxhgknde5jnjgg6yegwuw8rklvh6cvyvzqkgwaj857cz7xt3u8mhxlh8xevud3vj5dvq6kpxqd4jftt5h4gcmf9qpj3e2nw87j9une3vu75ahewdrqg7avfquw79fva59f8f3xpmk6lpmlkx9x7ejaw97f8nu86r2yhaepr50cdew82c3fmpnma2gr5vatjy3luqsyf8fpqp2zrjzcymemt3f3t99rn689ucyaj8vc2eapgw4knjyaque29hk3t7swcdvrwcf5myg33ghmg2s8xrqjwzeghzmqq68278lrw5rxn4jf3y93z7ztuwz67s0qa5lldcqe44qsshpuxx36dmna5cn7yy5v5f449gf26hygmj6qk8hm7rkvv44w3cu9fdv7sq0hqy67p3tvyxc8fl640z7pdsjfraznvqpnvcepggdnf3qypgs8vu82wsj2yd8nkhfv6sv6xs3wf5d7nkqsd5k8ehk7dtfqnsvcz26yazc32cv669qn7dhxr25j0etmmz7xh8azj7dn0d4u309m0rc2yhfegds60smuqtxn4l4nhmdqj9x6se4sultl5cwy4qja66cvnjz6mqwqet4n5zcswywqd6gcpec4q2vek9g4086ys4x35hwa47dk3zj2m03yuqz7ap66dah3r73j96q00cwmqw0lxvvqq4u0kvt6vrc0urd2hfhrxkrkmr9yx48uw94vmnjyq7sgyc0szkyuq07cjhg0fhx5z5mr9ua24wx9qnh32cjult3mu8kzhlj7se2nm4jr937j64656q7vp98dh9dhvlge8p02ejse5r0nsk22aa5cexvuqcaulnxw690vm3vdagdckfwps06jjd49kd4ls4jkf0nxkhqx2rm73pcepr4u6xjxw2fhjptk95tt0rq2ramq57lfg3sw3tsee2af355lt53w4f5wmpcvctsntyl2sp8m04l3nds7acv4uqnznudmkasgdf7l9df4484ym2njjzy0c26v2zv7pkv30f06uuptdvuxmgnuqcgd4els7gehp0fwxam0vskt34e3z3kfft6kkdz2c7ftn3dcvz5wvpwqf8458ade6995vdkxkalqzfs5epjfnn3c27mnzlx6cv5fhlephxpa3mj3hu6wafd8em8jhzcguru797p6m2fes55ha23putxrtly4wufl6rpp3ydta57zcxl40pvhpps7sgr7zc2cvz57xdlxpvclsjdgp5q3up9tu5csfdkaa762mk7zrqad93506l0kj", Network::Alpha).unwrap(), NeptuneCoins::new(1337)),
            (ReceivingAddress::from_bech32m("nolgam1hfgnle0202fgz75wh5cqpxkzz29775pqudt9z9v0s6h2e3gkfqkgv3xqn4xfq809k880cspd4dw4mmmcy3dus2pyxwcfysle3hsw2qc62qk3d4hesv56q45d539s28e267mzdvcgyrnwuz358edzjcpzwkep3wxccxrss7qqj0806uff26waqg2z37g7g8erew0eyaq83lv4wuqhql89rsmz8gxhwna4r2s48vww94vyvw9xllydqfygc8890qhhxa2sr3p70p3rdkgt7xuulh66uarnd3l0e0wl2ld7hw4klalacw6yk0u29g0eqx2vsvz29krw9s5n8vfckazhmx4f7393lxwp8aje47j9fpnvlgqr9p990qrmhx9vk8pvfc70wec3fn2c7sz9mttpzv74084pzcmrycqwd5c6qv95ks8duxv325yay48xs9zlgtf9d0zleneemhwzwknsct7ea7quj00359urmuvsvrftvht9wmhtkdzwe6jr6jqvjyn8ew8artcme97smx5dxy4m8yug67xcpfz8chtx0t7eerce7gtpfdn0cryx4s2erhedxk883jykck9ryj3akv7pqrvyldy3ruckgpcm9g6w6fc75yt9g466wemkhftx7tp6uskcvjnvrpn6wzadp44qmua3c23c3pylpdcx0wsv5vl3rspn36zwuzmzpma9ndpppa4dluqag8kfw7xj055szhrf4lsyquxmxq2efp74y75e535y3mgvhqgultm2f7m33hc6vk8ztymz59efth64msyqkmqx5mshm42kqwhqvznkw0ezmh22lfcd6fsh0l4gdujnmz7yfvyfdajkx80j87zmz2nhnv50qdpqjkrhem9ankxw3f06yhc6m5ltfeyhm7nq98glcgtljwss2r7m0gl8d8p2hlesa6cm0ld2y8s7prhz8gywl20dh89ve7qknljygdd5w7l5ueykmz736atgg5vevludsdut9xamwmtsye0fca6c2tl0ne8wpnsdljttt97qrf0mxemdm90v44v9wqet0utf4x0ahqqrlhf647rytaesj6j7dzqpan03za3lkqfcx7pymngzwl29rm62yklh3p884e5hz6qdwfaz98lsq9lke5ntmg2w55xvraleegkn6nftdr2ztgs58zfndpzafqs6v7tcm75hapw6hptzqwnpfwcvw38ghru55y003xm76tsd2fe6565fv5snakw74act2k2lsfg8ntaxf62ksgusdt9a6pw7mfypv2n2y9phddpj62yg93fxyqcujxw7vjced4eteendff28nmwmr3mtclyqhrry8palcsekavj8dstmkgezw6l3vq98p254mkxxye2uumaw8zh2mzvuqsgn0jfkymq76rlvx2d8e2xe6tv34vtpr09lhlehh4cwl48mjq7h0pnwlkrxyf0k0scw3szrc6wqg4hnc9whpx3whmdd2neme9j8lzauzyq45fqks6qt5vmq7lqx0a0flurpleyaq5466dzajma5vlqlgaggxxs3r3glumrpqtu6pd5mnemnuuc6f4gdjr65jdy3em8whcxwjnex6smkrxv5kjdag7cx0j8m8cg26hkkwyra9a0xqauzu0vaxd5qnx6cpm0w68evt4v960axzzuaevkagsyft9df6tnq0g2yqm7w7frht8wsxy4s0p227psd92d3vd5t45zesrvny4lvfvkn0cnwyf7p60gtx3er45xs4u4zy2ntrkx64elmp8k4v6kv0w8sh76ychxn384m4hhrrg523ex6ux0fhs63fkk7r68p3jlm4wcmxvxt872gg930m30l5v9vw6g4txy84w2wvvh7vxdu7tq50we9yp7x0wv2f6kfe4dthcmp2sjxf5l2myhegj3u8uz0m652flmsdyu57f8ncszjtkzh44afw4quw4j7dx6m322p6q2nkcw2x0n5lxwr3u2qd7t2rc28c4wgzdfgl2qvqpf95z0uv5m7p9crhl2hjzje3zqgyzgxxd4zku3yuhmj4saqeff78r78fth39p6mryyk95m4r76x30etzf7mcaudthhzrw3ae2fts576kh0c5ksnnzamtyr8ak6t4dn86a5zupn4kv426wwy7j688aasxupw7nu9qvkagm2a44ssk88ffyjxznrjtdln45vejx5ghaewzju6qze507shwtmu8evxcxv7h4axwqyvufxrvsmw3n88600af973r3k3nn3crs063j7ncc36luckfgajmqu6qtxt5emyzzmfy4pp9u4swfqtacaqgqmfjmmzansw9qv7zmhzz0wzllcv8a82f6apyt5kgrkdxg58a854rc4940gq2wy6y8lwtrkp3uf9fgms64d5d6990jzrfcr7xdkwp3fh8p66q7mfu03wpk0jzulqnu7dt6qppal3gkxhk384dvh8makve69vht6lcn032f2pavs0x4uq94s2lycmuvrevv6jrf76c90e6juz0q5w3744me7xagrunr3qpg4p8pqmyae4d7gzz8wr2znqg8wp32n2zdegz3qsmct9rhc4w5ne97epn5xdzzfa3rnqqllfqdu2672pk9a5uqldewz3v5haxnrxdhl3h52srthlv3c8ythj4m692rp74mzl2wx3svw864weq8437gqq9ejkhmkqnpzwzq7mtgp6c9r6sw2qqz4u2688wqet3yxf8rdqe0l9r9glhl5jq4arrx5f45k6l79mn9x44mmersqcrk3kmyfnptqe023rk5349a878n6qymd36tp6pvpxyxnuksyvw6yetyk4kvth6yqx5ke0q2v5ka49ewh787pgz4cnsvc2plyjwky8nurldynf44e9h0vaeukdk7xhs3slfydmmy2y84lez9uwqkj76e68fsws4g4jjlck902hs6ymmuhw52th2e82myf77wcxph7ka75qhhd4x35gd2lz8rajhjnfnns65gp3kqmwmq52st273jx7xs0xpper2s0jawgs38s3x8ggn3nk7a8k3dwlr7hry38xgyyjpvm6qlwvdyv5sau6a0rdyumrmut6uuxk90jqm2s4mp9u5rnyasedzeugegcygj72u29t7t2swvdr4mwrynryusp24d4s3l8ppj7tpks2nj8a3tlwzqh2feew6swzkf839lczs5rq4pcvmsgcy5ck5x0p759vwzqxwn7trtg0x7grfzpdc50x8zudrwad7fye8ca2zc7f8m689e34u003wc5dzs32cd8mxljkdpt4elasxcxse08948zeq239k8c442yffxz85uyqzcjyc86rfw3g79x5h3zkjq35t9v8vwskawag2vzmjtrmn4knst75kf3pfgt3mnkavs3fgyq9nfut343nmne8cct4uhj8zp0hrplpwf65kjvw8gqwstyg0gqejy4aur5", Network::Alpha).unwrap(), NeptuneCoins::new(42)),
        ]
    }

    pub fn premine_utxos(network: Network) -> Vec<Utxo> {
        let mut utxos = vec![];
        for (receiving_address, amount) in Self::premine_distribution(network) {
            // generate utxo
            let mut utxo = Utxo::new_native_coin(receiving_address.lock_script(), amount);
            let six_months = Timestamp::months(6);
            utxo.coins
                .push(TimeLock::until(network.launch_date() + six_months));
            utxos.push(utxo);
        }
        utxos
    }

    pub fn new(header: BlockHeader, body: BlockBody, block_type: BlockType) -> Self {
        let kernel = BlockKernel { body, header };
        Self {
            digest: Default::default(), // calc'd in hash()
            kernel,
            block_type,
        }
    }

    /// helper fn to generate a BlockType::Standard enum variant representing a standard Block (non-genesis).
    ///
    /// note: This consolidates creation of ProofType::Unimplemented
    /// into one place, so that once all Proofs are implemented we
    /// can easily remove ProofType::Unimplemented.  We will
    /// still need to make this proof param non optional though.
    pub fn mk_std_block_type(proof: Option<Proof>) -> BlockType {
        let proof_type = match proof {
            Some(p) => ProofType::Proof(p),
            None => ProofType::Unimplemented,
        };
        BlockType::Standard(proof_type)
    }

    /// Merge a transaction into this block's transaction.
    /// The mutator set data must be valid in all inputs.
    ///
    /// note: this causes block digest to change.
    pub async fn accumulate_transaction(
        &mut self,
        transaction: Transaction,
        previous_mutator_set_accumulator: &MutatorSetAccumulator,
    ) {
        // merge transactions
        let merged_timestamp = max::<Timestamp>(
            self.kernel.header.timestamp,
            max::<Timestamp>(
                self.kernel.body.transaction.kernel.timestamp,
                transaction.kernel.timestamp,
            ),
        );
        let new_transaction = self
            .kernel
            .body
            .transaction
            .clone()
            .merge_with(transaction.clone());

        // accumulate mutator set updates
        // Can't use the current mutator sat accumulator because it is in an in-between state.
        let mut new_mutator_set_accumulator = previous_mutator_set_accumulator.clone();
        let mutator_set_update = MutatorSetUpdate::new(
            new_transaction.kernel.inputs.clone(),
            new_transaction.kernel.outputs.clone(),
        );

        // Apply the mutator set update to get the `next_mutator_set_accumulator`
        mutator_set_update
            .apply_to_accumulator(&mut new_mutator_set_accumulator)
            .expect("Mutator set mutation must work");

        let block_body: BlockBody = BlockBody {
            transaction: new_transaction,
            mutator_set_accumulator: new_mutator_set_accumulator.clone(),
            lock_free_mmr_accumulator: self.kernel.body.lock_free_mmr_accumulator.clone(),
            block_mmr_accumulator: self.kernel.body.block_mmr_accumulator.clone(),
            uncle_blocks: self.kernel.body.uncle_blocks.clone(),
        };

        let block_header = BlockHeader {
            version: self.kernel.header.version,
            height: self.kernel.header.height,
            prev_block_digest: self.kernel.header.prev_block_digest,
            timestamp: merged_timestamp,
            nonce: self.kernel.header.nonce,
            max_block_size: self.kernel.header.max_block_size,
            proof_of_work_line: self.kernel.header.proof_of_work_line,
            proof_of_work_family: self.kernel.header.proof_of_work_family,
            difficulty: self.kernel.header.difficulty,
        };

        self.kernel.body = block_body;
        self.kernel.header = block_header;
        self.unset_digest();
    }

    /// Verify a block. It is assumed that `previous_block` is valid.
    /// Note that this function does **not** check that the PoW digest is below the threshold.
    /// That must be done separately by the caller.
    pub(crate) fn is_valid(&self, previous_block: &Block, now: Timestamp) -> bool {
        self.is_valid_extended(previous_block, now, None)
    }

    /// like is_valid() but also allows specifying a custom target_block_interval.
    pub(crate) fn is_valid_extended(
        &self,
        previous_block: &Block,
        now: Timestamp,
        target_block_interval: Option<u64>,
    ) -> bool {
        // The block value doesn't actually change. Some function calls just require
        // mutable references because that's how the interface was defined for them.
        let block_copy = self.to_owned();
        // What belongs here are the things that would otherwise
        // be verified by the block validity proof.

        // 0. `previous_block` is consistent with current block
        //   a) Block height is previous plus one
        //   b) Block header points to previous block
        //   d) Block timestamp is greater than previous block timestamp
        //   e) Target difficulty, and other control parameters, were adjusted correctly
        //   f) Block timestamp is less than host-time (utc) + 2 hours.
        // 1. The transaction is valid.
        // 1'. All transactions are valid.
        //   a) verify that MS membership proof is valid, done against previous `mutator_set_accumulator`,
        //   b) Verify that MS removal record is valid, done against previous `mutator_set_accumulator`,
        //   c) Verify that all removal records have unique index sets
        //   d) verify that adding `mutator_set_update` to previous `mutator_set_accumulator`
        //      gives `next_mutator_set_accumulator`,
        //   e) transaction timestamp <= block timestamp
        //   f) transaction coinbase <= miner reward
        //   g) transaction is valid (internally consistent)

        // 0.a) Block height is previous plus one
        if previous_block.kernel.header.height.next() != block_copy.kernel.header.height {
            warn!(
                "Block height ({}) does not match previous height plus one ({})",
                block_copy.kernel.header.height,
                previous_block.kernel.header.height.next()
            );
            return false;
        }

        // 0.b) Block header points to previous block
        if previous_block.hash() != block_copy.kernel.header.prev_block_digest {
            warn!("Hash digest does not match previous digest");
            return false;
        }

        // 0.c) Verify correct addition to block MMR
        let mut mmra = previous_block.kernel.body.block_mmr_accumulator.clone();
        mmra.append(previous_block.hash());
        if mmra != self.kernel.body.block_mmr_accumulator {
            warn!("Block MMRA was not updated correctly");
            return false;
        }

        // 0.d) Block timestamp is greater than (or equal to) that of previous block
        if previous_block.kernel.header.timestamp > block_copy.kernel.header.timestamp {
            warn!(
                "Block's timestamp ({}) should be greater than or equal to that of previous block ({})\nprevious <= current ?? {}",
                block_copy.kernel.header.timestamp,
                previous_block.kernel.header.timestamp,
                previous_block.kernel.header.timestamp <= block_copy.kernel.header.timestamp
            );
            return false;
        }

        // 0.e) Target difficulty, and other control parameters, were updated correctly
        if block_copy.kernel.header.difficulty
            != Self::difficulty_control(
                previous_block,
                block_copy.kernel.header.timestamp,
                target_block_interval,
            )
        {
            warn!(
                "Value for new difficulty is incorrect.  actual: {},  expected: {}",
                block_copy.kernel.header.difficulty,
                Self::difficulty_control(
                    previous_block,
                    block_copy.kernel.header.timestamp,
                    target_block_interval
                )
            );
            return false;
        }

        // 0.f) Block timestamp is less than host-time (utc) + 2 hours.
        let future_limit = now + Timestamp::hours(2);
        if block_copy.kernel.header.timestamp >= future_limit {
            warn!("block time is too far in the future");
            return false;
        }

        // 1.b) Verify validity of removal records: That their MMR MPs match the SWBF, and
        // that at least one of their listed indices is absent.
        for removal_record in block_copy.kernel.body.transaction.kernel.inputs.iter() {
            if !previous_block
                .kernel
                .body
                .mutator_set_accumulator
                .can_remove(removal_record)
            {
                warn!("Removal record cannot be removed from mutator set");
                return false;
            }
        }

        // 1.c) Verify that the removal records do not contain duplicate `AbsoluteIndexSet`s
        let mut absolute_index_sets = block_copy
            .kernel
            .body
            .transaction
            .kernel
            .inputs
            .iter()
            .map(|removal_record| removal_record.absolute_indices.to_vec())
            .collect_vec();
        absolute_index_sets.sort();
        absolute_index_sets.dedup();
        if absolute_index_sets.len() != block_copy.kernel.body.transaction.kernel.inputs.len() {
            warn!("Removal records contain duplicates");
            return false;
        }

        // 1.d) Verify that the two mutator sets, the one from the current block and the
        // one from the previous, are consistent with the transactions.
        // Construct all the addition records for all the transaction outputs. Then
        // use these addition records to insert into the mutator set.
        let mutator_set_update = MutatorSetUpdate::new(
            block_copy.kernel.body.transaction.kernel.inputs.clone(),
            block_copy.kernel.body.transaction.kernel.outputs.clone(),
        );
        let mut ms = previous_block.kernel.body.mutator_set_accumulator.clone();
        let ms_update_result = mutator_set_update.apply_to_accumulator(&mut ms);
        match ms_update_result {
            Ok(()) => (),
            Err(err) => {
                warn!("Failed to apply mutator set update: {}", err);
                return false;
            }
        };

        // Verify that the locally constructed mutator set matches that in the received
        // block's body.
        if ms.hash() != block_copy.kernel.body.mutator_set_accumulator.hash() {
            warn!("Reported mutator set does not match calculated object.");
            debug!(
                "From Block\n{:?}. \n\n\nCalculated\n{:?}",
                block_copy.kernel.body.mutator_set_accumulator, ms
            );
            return false;
        }

        // 1.e) verify that the transaction timestamp is less than or equal to the block's timestamp.
        if block_copy.kernel.body.transaction.kernel.timestamp > block_copy.kernel.header.timestamp
        {
            warn!(
                "Transaction timestamp ({}) is is larger than that of block ({})",
                block_copy.kernel.body.transaction.kernel.timestamp,
                block_copy.kernel.header.timestamp
            );
            return false;
        }

        // 1.f) Verify that the coinbase claimed by the transaction does not exceed
        // the allowed coinbase based on block height, epoch, etc., and fee
        let miner_reward: NeptuneCoins = Self::get_mining_reward(block_copy.kernel.header.height)
            + self.kernel.body.transaction.kernel.fee;
        if let Some(claimed_reward) = block_copy.kernel.body.transaction.kernel.coinbase {
            if claimed_reward > miner_reward {
                warn!("Block is invalid because the claimed miner reward is too high relative to current network parameters.");
                return false;
            }
        }

        // 1.g) Verify transaction, but without relating it to the blockchain tip (that was done above).
        if !block_copy.kernel.body.transaction.is_valid() {
            warn!("Invalid transaction found in block");
            return false;
        }

        // 2. accumulated proof-of-work was computed correctly
        //  - look two blocks back, take proof_of_work_line
        //  - look 1 block back, estimate proof-of-work
        //  - add -> new proof_of_work_line
        //  - look two blocks back, take proof_of_work_family
        //  - look at all uncles, estimate proof-of-work
        //  - add -> new proof_of_work_family

        // 3. variable network parameters are computed correctly
        // 3.a) target_difficulty <- pow_line
        // 3.b) max_block_size <- difference between `pow_family[n-2] - pow_line[n-2] - (pow_family[n] - pow_line[n])`

        // 4. for every uncle
        //  4.1. verify that uncle's prev_block_digest matches with parent's prev_block_digest
        //  4.2. verify that all uncles' hash are below parent's target_difficulty

        true
    }

    /// Determine if the the proof-of-work puzzle was solved correctly. Specifically,
    /// compare the hash of the current block against the difficulty determined by
    /// the previous.
    pub fn has_proof_of_work(&self, previous_block: &Block) -> bool {
        self.hash() <= Self::difficulty_to_digest_threshold(previous_block.kernel.header.difficulty)
    }

    /// Converts `difficulty` to type `Digest` so that the hash of a block can be
    /// tested against the target difficulty using `<`. The unit of `difficulty`
    /// is expected number of hashes for solving the proof-of-work puzzle.
    pub fn difficulty_to_digest_threshold(difficulty: U32s<5>) -> Digest {
        assert!(!difficulty.is_zero(), "Difficulty cannot be less than 1");

        let difficulty_as_bui: BigUint = difficulty.into();
        let max_threshold_as_bui: BigUint =
            Digest([BFieldElement::new(BFieldElement::MAX); DIGEST_LENGTH]).into();
        let threshold_as_bui: BigUint = max_threshold_as_bui / difficulty_as_bui;

        threshold_as_bui.try_into().unwrap()
    }

    /// Control system for block difficulty. This function computes the new block's
    /// difficulty from its timestamp and the previous block. It is a PID controller
    /// (with i=d=0) regulating the block interval by tuning the difficulty.
    /// We assume that the block timestamp is valid.
    pub fn difficulty_control(
        old_block: &Block,
        new_timestamp: Timestamp,
        target_block_interval: Option<u64>,
    ) -> U32s<TARGET_DIFFICULTY_U32_SIZE> {
        // no adjustment if the previous block is the genesis block
        if old_block.kernel.header.height.is_genesis() {
            return old_block.kernel.header.difficulty;
        }

        let target_block_interval = target_block_interval.unwrap_or(TARGET_BLOCK_INTERVAL);

        // otherwise, compute PID control signal
        let t = new_timestamp - old_block.kernel.header.timestamp;

        let new_error = t.0.value() as i64 - target_block_interval as i64;
        let adjustment = -new_error / 100;
        let absolute_adjustment = abs(adjustment) as u64;
        let adjustment_is_positive = adjustment >= 0;
        let adj_hi = (absolute_adjustment >> 32) as u32;
        let adj_lo = absolute_adjustment as u32;
        let adjustment_u32s =
            U32s::<TARGET_DIFFICULTY_U32_SIZE>::new([adj_lo, adj_hi, 0u32, 0u32, 0u32]);

        if adjustment_is_positive {
            old_block.kernel.header.difficulty + adjustment_u32s
        } else if adjustment_u32s > old_block.kernel.header.difficulty - MINIMUM_DIFFICULTY.into() {
            MINIMUM_DIFFICULTY.into()
        } else {
            old_block.kernel.header.difficulty - adjustment_u32s
        }
    }
}

#[cfg(test)]
mod block_tests {
    use rand::random;
    use rand::thread_rng;
    use rand::Rng;
    use strum::IntoEnumIterator;
    use tracing_test::traced_test;

    use crate::config_models::network::Network;
    use crate::database::storage::storage_schema::SimpleRustyStorage;
    use crate::database::NeptuneLevelDb;
    use crate::models::blockchain::transaction::TxOutput;
    use crate::models::state::wallet::WalletSecret;
    use crate::tests::shared::make_mock_block;
    use crate::tests::shared::make_mock_block_with_valid_pow;
    use crate::tests::shared::mock_genesis_global_state;
    use crate::util_types::mutator_set::archival_mmr::ArchivalMmr;

    use super::*;

    async fn merge_transaction() -> (Block, Block, Block) {
        let mut rng = thread_rng();
        // We need the global state to construct a transaction. This global state
        // has a wallet which receives a premine-UTXO.
        let network = Network::RegTest;
        let mut global_state_lock =
            mock_genesis_global_state(network, 2, WalletSecret::devnet_wallet()).await;
        let spending_key = global_state_lock
            .lock_guard()
            .await
            .wallet_state
            .wallet_secret
            .nth_generation_spending_key_for_tests(0);
        let address = spending_key.to_address();
        let other_wallet_secret = WalletSecret::new_random();
        let other_address = other_wallet_secret
            .nth_generation_spending_key_for_tests(0)
            .to_address();
        let genesis_block = Block::genesis_block(network);

        let (block_1, _, _) = make_mock_block(&genesis_block, None, address, rng.gen());
        let now = genesis_block.kernel.header.timestamp;
        let seven_months = Timestamp::months(7);
        assert!(
            block_1.is_valid(&genesis_block, now),
            "Block 1 must be valid with only coinbase output"
        );

        // create a new transaction, merge it into block 1 and check that block 1 is still valid
        let new_utxo = Utxo::new_native_coin(other_address.lock_script(), NeptuneCoins::new(10));
        let reciever_data =
            TxOutput::fake_address(new_utxo, random(), other_address.privacy_digest);
        let (new_tx, expected_utxos) = global_state_lock
            .lock_guard()
            .await
            .create_transaction_test_wrapper(
                vec![reciever_data],
                NeptuneCoins::new(1),
                now + seven_months,
            )
            .await
            .unwrap();
        assert!(new_tx.is_valid(), "Created tx must be valid");

        // inform wallet of any expected utxos from this tx.
        global_state_lock
            .lock_guard_mut()
            .await
            .add_expected_utxos_to_wallet(expected_utxos)
            .await
            .unwrap();

        let mut block_1_merged = block_1.clone();

        block_1_merged
            .accumulate_transaction(new_tx, &genesis_block.kernel.body.mutator_set_accumulator)
            .await;

        (genesis_block, block_1, block_1_merged)
    }

    // #[traced_test]
    #[test]
    fn test_difficulty_control_matches() {
        let mut rng = thread_rng();
        let network = Network::RegTest;

        let a_wallet_secret = WalletSecret::new_random();
        let a_recipient_address = a_wallet_secret
            .nth_generation_spending_key_for_tests(0)
            .to_address();

        for multiplier in [10, 100, 1000, 10000, 100000, 1000000] {
            let mut block_prev = Block::genesis_block(network);
            let mut now = block_prev.kernel.header.timestamp;

            for i in (0..100).step_by(1) {
                let duration = i as u64 * multiplier;
                now = now + Timestamp::millis(duration);

                let (block, _, _) =
                    make_mock_block(&block_prev, Some(now), a_recipient_address, rng.gen());

                println!(
                    "height: {}, now: {}",
                    block.kernel.header.height,
                    now.standard_format()
                );

                let control =
                    Block::difficulty_control(&block_prev, block.kernel.header.timestamp, None);
                assert_eq!(block.kernel.header.difficulty, control);

                block_prev = block;
            }
        }
    }

    #[traced_test]
    #[tokio::test]
    async fn merge_transaction_test() {
        let (genesis_block, _, block_1) = merge_transaction().await;
        let now = genesis_block.kernel.header.timestamp;
        let seven_months = Timestamp::months(7);

        assert!(
            block_1.is_valid(&genesis_block, now + seven_months),
            "Block 1 must be valid after adding a transaction; previous mutator set hash: {} and next mutator set hash: {}",
            genesis_block.kernel
                .body
                .mutator_set_accumulator
                .hash(),
            block_1.kernel
                .body
                .mutator_set_accumulator
                .hash()
        );

        // Sanity checks
        assert_eq!(
            3,
            block_1.kernel.body.transaction.kernel.outputs.len(),
            "New block must have three outputs: coinbase, transaction, and change"
        );
        assert_eq!(
            1,
            block_1.kernel.body.transaction.kernel.inputs.len(),
            "New block must have one input: spending of genesis UTXO"
        );
    }

    #[test]
    fn difficulty_to_threshold_test() {
        // Verify that a difficulty of 2 accepts half of the digests
        let difficulty: u32 = 2;
        let difficulty_u32s = U32s::<5>::from(difficulty);
        let threshold_for_difficulty_two: Digest =
            Block::difficulty_to_digest_threshold(difficulty_u32s);

        for elem in threshold_for_difficulty_two.values() {
            assert_eq!(BFieldElement::MAX / u64::from(difficulty), elem.value());
        }

        // Verify that a difficulty of BFieldElement::MAX accepts all digests where the last BFieldElement is zero
        let some_difficulty = U32s::<5>::new([1, u32::MAX, 0, 0, 0]);
        let some_threshold_actual: Digest = Block::difficulty_to_digest_threshold(some_difficulty);

        let bfe_max_elem = BFieldElement::new(BFieldElement::MAX);
        let some_threshold_expected = Digest::new([
            bfe_max_elem,
            bfe_max_elem,
            bfe_max_elem,
            bfe_max_elem,
            BFieldElement::zero(),
        ]);

        assert_eq!(0u64, some_threshold_actual.values()[4].value());
        assert_eq!(some_threshold_actual, some_threshold_expected);
        assert_eq!(bfe_max_elem, some_threshold_actual.values()[3]);
    }

    #[test]
    fn block_with_wrong_mmra_is_invalid() {
        let mut rng = thread_rng();
        let network = Network::RegTest;
        let genesis_block = Block::genesis_block(network);

        let a_wallet_secret = WalletSecret::new_random();
        let a_recipient_address = a_wallet_secret
            .nth_generation_spending_key_for_tests(0)
            .to_address();
        let (mut block_1, _, _) =
            make_mock_block_with_valid_pow(&genesis_block, None, a_recipient_address, rng.gen());

        block_1.kernel.body.block_mmr_accumulator = MmrAccumulator::new(vec![]);
        let timestamp = genesis_block.kernel.header.timestamp;

        assert!(!block_1.is_valid(&genesis_block, timestamp));
    }

    #[traced_test]
    #[test]
    fn block_with_far_future_timestamp_is_invalid() {
        let mut rng = thread_rng();
        let network = Network::RegTest;
        let genesis_block = Block::genesis_block(network);
        let mut now = genesis_block.kernel.header.timestamp;

        let a_wallet_secret = WalletSecret::new_random();
        let a_recipient_address = a_wallet_secret
            .nth_generation_spending_key_for_tests(0)
            .to_address();
        let (mut block_1, _, _) =
            make_mock_block_with_valid_pow(&genesis_block, None, a_recipient_address, rng.gen());

        // Set block timestamp 1 hour in the future.  (is valid)
        let future_time1 = now + Timestamp::hours(1);
        block_1.kernel.header.timestamp = future_time1;
        assert!(block_1.is_valid(&genesis_block, now));

        now = block_1.kernel.header.timestamp;

        // Set block timestamp 2 hours - 1 sec in the future.  (is valid)
        let future_time2 = now + Timestamp::hours(2) - Timestamp::seconds(1);
        block_1.kernel.header.timestamp = future_time2;
        assert!(block_1.is_valid(&genesis_block, now));

        // Set block timestamp 2 hours + 10 secs in the future. (not valid)
        let future_time3 = now + Timestamp::hours(2) + Timestamp::seconds(10);
        block_1.kernel.header.timestamp = future_time3;
        assert!(!block_1.is_valid(&genesis_block, now));

        // Set block timestamp 2 days in the future. (not valid)
        let future_time4 = now + Timestamp::seconds(86400 * 2);
        block_1.kernel.header.timestamp = future_time4;
        assert!(!block_1.is_valid(&genesis_block, now));
    }

    #[tokio::test]
    async fn can_prove_block_ancestry() {
        let mut rng = thread_rng();
        let network = Network::RegTest;
        let genesis_block = Block::genesis_block(network);
        let mut blocks = vec![];
        blocks.push(genesis_block.clone());
        let db = NeptuneLevelDb::open_new_test_database(true, None, None, None)
            .await
            .unwrap();
        let mut storage = SimpleRustyStorage::new(db);
        let ammr_storage = storage.schema.new_vec::<Digest>("ammr-blocks-0").await;
        let mut ammr: ArchivalMmr<Hash, _> = ArchivalMmr::new(ammr_storage).await;
        ammr.append(genesis_block.hash()).await;
        let mut mmra = MmrAccumulator::new(vec![genesis_block.hash()]);

        for i in 0..55 {
            let wallet_secret = WalletSecret::new_random();
            let recipient_address = wallet_secret
                .nth_generation_spending_key_for_tests(0)
                .to_address();
            let (new_block, _, _) =
                make_mock_block(blocks.last().unwrap(), None, recipient_address, rng.gen());
            if i != 54 {
                ammr.append(new_block.hash()).await;
                mmra.append(new_block.hash());
                assert_eq!(
                    ammr.to_accumulator_async().await.bag_peaks(),
                    mmra.bag_peaks()
                );
            }
            blocks.push(new_block);
        }

        let last_block_mmra = blocks.last().unwrap().body().block_mmr_accumulator.clone();
        assert_eq!(mmra, last_block_mmra);

        let index = thread_rng().gen_range(0..blocks.len() - 1);
        let block_digest = blocks[index].hash();
        let membership_proof = ammr.prove_membership_async(index as u64).await;
        let v = membership_proof.verify(
            &last_block_mmra.get_peaks(),
            block_digest,
            last_block_mmra.count_leaves(),
        );
        assert!(
            v,
            "peaks: {} ({}) leaf count: {} index: {} path: {} number of blocks: {} leaf index: {}",
            last_block_mmra.get_peaks().iter().join(","),
            last_block_mmra.get_peaks().len(),
            last_block_mmra.count_leaves(),
            membership_proof.leaf_index,
            membership_proof.authentication_path.iter().join(","),
            blocks.len(),
            membership_proof.leaf_index
        );
        assert_eq!(last_block_mmra.count_leaves(), blocks.len() as u64 - 1);
    }

    #[test]
    fn test_premine_size() {
        // 831600 = 42000000 * 0.0198
        // where 42000000 is the asymptotical limit of the token supply
        // and 1.98% is the relative size of the premine
        for network in Network::iter() {
            let premine_max_size = NeptuneCoins::new(831600);
            let total_premine = Block::premine_distribution(network)
                .iter()
                .map(|(_receiving_address, amount)| *amount)
                .sum::<NeptuneCoins>();

            assert!(total_premine <= premine_max_size);
        }
    }

    /// This module has tests that verify a block's digest
    /// is always in a correct state.
    ///
    /// All operations that create or modify a Block should
    /// have a test here.
    mod digest_encapsulation {
        use super::*;

        // test: verify clone + modify does not change original.
        //
        // note: a naive impl that derives Clone on `Block` containing
        //       Arc<Mutex<Option<Digest>>> would link the digest in the clone
        #[test]
        fn clone_and_modify() {
            let gblock = Block::genesis_block(Network::RegTest);
            let g_hash = gblock.hash();

            let mut g2 = gblock.clone();
            assert_eq!(gblock.hash(), g_hash);
            assert_eq!(gblock.hash(), g2.hash());

            g2.set_header_nonce([1u8.into(), 1u8.into(), 1u8.into()]);
            assert_ne!(gblock.hash(), g2.hash());
            assert_eq!(gblock.hash(), g_hash);
        }

        // test: verify digest is correct after Block::new().
        #[test]
        fn new() {
            let gblock = Block::genesis_block(Network::RegTest);
            let g2 = gblock.clone();

            let block = Block::new(g2.kernel.header, g2.kernel.body, g2.block_type);
            assert_eq!(gblock.hash(), block.hash());
        }

        // test: verify digest changes after nonce is updated.
        #[test]
        fn set_header_nonce() {
            let gblock = Block::genesis_block(Network::RegTest);
            let mut rng = thread_rng();

            let mut new_block = gblock.clone();
            new_block.set_header_nonce(rng.gen());
            assert_ne!(gblock.hash(), new_block.hash());
        }

        // test: verify set_block() copies source digest
        #[test]
        fn set_block() {
            let gblock = Block::genesis_block(Network::RegTest);
            let mut rng = thread_rng();

            let mut unique_block = gblock.clone();
            unique_block.set_header_nonce(rng.gen());

            let mut block = gblock.clone();
            block.set_block(unique_block.clone());

            assert_eq!(unique_block.hash(), block.hash());
            assert_ne!(unique_block.hash(), gblock.hash());
        }

        // test: verify digest is the same after conversion from
        //       TransferBlock and back.
        #[tokio::test]
        async fn from_transfer_block() {
            // note: we have to generate a block becau            // TransferBlock::into() will panic if it
            // encounters the genesis block.
            let global_state_lock =
                mock_genesis_global_state(Network::RegTest, 2, WalletSecret::devnet_wallet()).await;
            let spending_key = global_state_lock
                .lock_guard()
                .await
                .wallet_state
                .wallet_secret
                .nth_generation_spending_key_for_tests(0);
            let address = spending_key.to_address();
            let mut rng = thread_rng();

            let gblock = Block::genesis_block(Network::RegTest);

            let (source_block, _, _) = make_mock_block(&gblock, None, address, rng.gen());

            let transfer_block = TransferBlock::from(source_block.clone());
            let new_block = Block::from(transfer_block);
            assert_eq!(source_block.hash(), new_block.hash());
        }

        // test: verify digest is correct after deserializing
        #[test]
        fn deserialize() {
            let gblock = Block::genesis_block(Network::RegTest);

            let bytes = bincode::serialize(&gblock).unwrap();
            let block: Block = bincode::deserialize(&bytes).unwrap();

            assert_eq!(gblock.hash(), block.hash());
        }

        // test: verify block digest changes after accumulating
        // a transaction into the block.
        #[tokio::test]
        async fn accumulate_transaction() {
            let (_, block1, block1_merged) = merge_transaction().await;

            // verify that digest changed after merge.
            assert_ne!(block1.hash(), block1_merged.hash());
        }

        // test: verify block digest matches after BFieldCodec encode+decode
        //       round trip.
        #[test]
        fn bfieldcodec_encode_and_decode() {
            let gblock = Block::genesis_block(Network::RegTest);

            let encoded: Vec<BFieldElement> = gblock.encode();
            let decoded: Block = *Block::decode(&encoded).unwrap();

            assert_eq!(gblock, decoded);
            assert_eq!(gblock.hash(), decoded.hash());
        }
    }
}
