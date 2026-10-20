// C ABI over ACL's header-only decompressor, used by the `acl` cargo feature (rig animation).
#include <acl/decompression/decompress.h>
#include <cstdint>
#include <cstdlib>
#include <cstring>

namespace
{
	// ACL reads compressed buffers in place and requires 16-byte alignment.
	struct aligned_copy
	{
		void* ptr = nullptr;
		aligned_copy(const uint8_t* data, size_t len)
		{
			ptr = std::aligned_alloc(16, (len + 15) / 16 * 16);
			if (ptr != nullptr)
				std::memcpy(ptr, data, len);
		}
		~aligned_copy() { std::free(ptr); }
		const acl::compressed_tracks* tracks() const { return static_cast<const acl::compressed_tracks*>(ptr); }
	};

	// Writes rotation xyzw, translation xyz and scale xyz: 10 floats per track.
	struct transform_writer final : public acl::track_writer
	{
		float* out;
		explicit transform_writer(float* out_) : out(out_) {}
		void RTM_SIMD_CALL write_rotation(uint32_t i, rtm::quatf_arg0 q) { rtm::quat_store(q, out + i * 10); }
		void RTM_SIMD_CALL write_translation(uint32_t i, rtm::vector4f_arg0 t) { rtm::vector_store3(t, out + i * 10 + 4); }
		void RTM_SIMD_CALL write_scale(uint32_t i, rtm::vector4f_arg0 s) { rtm::vector_store3(s, out + i * 10 + 7); }
	};

	struct float_writer final : public acl::track_writer
	{
		float* out;
		explicit float_writer(float* out_) : out(out_) {}
		void RTM_SIMD_CALL write_float1(uint32_t i, rtm::scalarf_arg0 v) { out[i] = rtm::scalar_cast(v); }
	};
}

extern "C" {

// Header of a compressed buffer. Returns 0 when valid, a negative error otherwise.
int fluorite_acl_info(const uint8_t* data, size_t len, uint32_t* num_tracks, uint32_t* num_samples, float* sample_rate, uint8_t* track_type)
{
	if (data == nullptr || len < 16)
		return -1;
	aligned_copy copy(data, len);
	if (copy.ptr == nullptr)
		return -2;
	const acl::compressed_tracks* tracks = copy.tracks();
	if (tracks->get_size() > len || tracks->is_valid(true).any())
		return -3;
	*num_tracks = tracks->get_num_tracks();
	*num_samples = tracks->get_num_samples_per_track();
	*sample_rate = tracks->get_sample_rate();
	*track_type = static_cast<uint8_t>(tracks->get_track_type());
	return 0;
}

// Decompress every sample of a transform (qvvf) buffer into
// out[sample * num_tracks * 10 + track * 10 + k]. `out_len` is in floats.
int fluorite_acl_transforms(const uint8_t* data, size_t len, float* out, size_t out_len)
{
	aligned_copy copy(data, len);
	const acl::compressed_tracks* tracks = copy.tracks();
	if (copy.ptr == nullptr || tracks->is_valid(true).any() || tracks->get_track_type() != acl::track_type8::qvvf)
		return -1;
	const uint32_t n = tracks->get_num_tracks();
	const uint32_t samples = tracks->get_num_samples_per_track();
	if (out_len < size_t(n) * samples * 10)
		return -2;
	acl::decompression_context<acl::debug_transform_decompression_settings> context;
	if (!context.initialize(*tracks))
		return -3;
	const float rate = tracks->get_sample_rate();
	for (uint32_t s = 0; s < samples; ++s)
	{
		transform_writer writer(out + size_t(s) * n * 10);
		context.seek(float(s) / rate, acl::sample_rounding_policy::nearest);
		context.decompress_tracks(writer);
	}
	return 0;
}

// Decompress every sample of a scalar float1 buffer into out[sample * num_tracks + track].
int fluorite_acl_floats(const uint8_t* data, size_t len, float* out, size_t out_len)
{
	aligned_copy copy(data, len);
	const acl::compressed_tracks* tracks = copy.tracks();
	if (copy.ptr == nullptr || tracks->is_valid(true).any() || tracks->get_track_type() != acl::track_type8::float1f)
		return -1;
	const uint32_t n = tracks->get_num_tracks();
	const uint32_t samples = tracks->get_num_samples_per_track();
	if (out_len < size_t(n) * samples)
		return -2;
	acl::decompression_context<acl::debug_scalar_decompression_settings> context;
	if (!context.initialize(*tracks))
		return -3;
	const float rate = tracks->get_sample_rate();
	for (uint32_t s = 0; s < samples; ++s)
	{
		float_writer writer(out + size_t(s) * n);
		context.seek(float(s) / rate, acl::sample_rounding_policy::nearest);
		context.decompress_tracks(writer);
	}
	return 0;
}

}
